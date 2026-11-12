"""Patch an isolated, freshly copied Windows desktop package; never touch the installed app."""
import argparse
import hashlib
import json
import os
import struct
from pathlib import Path

PANEL = Path(__file__).with_name('background-panel.js')
COMPOSER = b'function o3(e){let t,n,r,i,s,l,d,c,u,m,h=(0,a.c)(54)'
INSERTION = b's9?null:yt,cx?(0,o.jsx)(o3,'
RAIL = b'rk=xl&&null==ix?(0,o.jsxs)(eH.l,'
STATE_MARKER = b'Dropping commandExecution/outputDelta for missing item'
OLD_GUID = b'e5768d8b-6936-4f45-b1ad-4c5fb414cb35'
NEW_GUID = b'67d18a29-bc8e-4356-bbe5-24859a20c913'


def digest(data):
    return hashlib.sha256(data).hexdigest()


def entries_for(root):
    entries = []
    def walk(node, path=''):
        for name, value in node.get('files', {}).items():
            part = f'{path}/{name}' if path else name
            if 'files' in value:
                walk(value, part)
            elif 'offset' in value:
                entries.append((part, value, int(value['offset'])))
    walk(root)
    return sorted(entries, key=lambda item: item[2])


def unique_member(old, base, entries, predicate, marker, label):
    found = []
    for path, entry, offset in entries:
        if not predicate(path):
            continue
        old.seek(base + offset)
        data = old.read(entry['size'])
        if marker in data:
            found.append((path, entry, data))
    if len(found) != 1:
        raise RuntimeError(f'{label}: expected one match, found {len(found)}; manual adaptation required')
    return found[0]


def replace_once(data, old, new, label):
    if data.count(old) != 1:
        raise RuntimeError(f'{label}: expected one anchor; manual adaptation required')
    return data.replace(old, new, 1)


def patch(app):
    archive = app / 'resources' / 'app.asar'
    executable = app / 'ChatGPT.exe'
    with archive.open('rb') as old:
        first, size, _, json_size = struct.unpack('<IIII', old.read(16))
        original_header = old.read(json_size)
        header = json.loads(original_header)
        entries = entries_for(header)
        base = 8 + size
        assets = lambda p: p.startswith('webview/assets/') and p.endswith('.js')
        main = lambda p: p.startswith('.vite/build/') and p.endswith('.js')
        composer_name, _, composer = unique_member(old, base, entries, assets, COMPOSER, 'composer')
        for required in (b'c=n("zTeN")', b'f=n("zd8w")', b'eX=n("Oj9r")'):
            if required not in composer:
                raise RuntimeError('composer dependency changed; manual adaptation required')
        composer = replace_once(composer, COMPOSER, PANEL.read_bytes() + COMPOSER, 'composer component')
        composer = replace_once(composer, INSERTION, b's9?null:yt,(0,o.jsx)(f.Suspense,{fallback:null,children:(0,o.jsx)(xkBackgroundPanel,{})}),cx?(0,o.jsx)(o3,', 'composer insertion')
        composer = replace_once(composer, RAIL, b'rk=null==ix?(0,o.jsxs)(eH.l,', 'composer rail')
        state_name, _, state = unique_member(old, base, entries, assets, STATE_MARKER, 'output updates')
        position = state.index(STATE_MARKER)
        flag = b'broadcastPatchesToFollowers:!1'
        at = state.find(flag, position, position + 600)
        if at < 0:
            raise RuntimeError('output broadcast flag changed; manual adaptation required')
        state = state[:at] + replace_once(state[at:at + len(flag)], flag, b'broadcastPatchesToFollowers:!0', 'output broadcast') + state[at + len(flag):]
        bootstrap_name, _, bootstrap = unique_member(old, base, entries, main, OLD_GUID, 'tray identity')
        bootstrap = replace_once(bootstrap, OLD_GUID, NEW_GUID, 'tray identity')
        # The panel references these bundled symbols; reject incompatible builds.
        _, _, terminal = unique_member(old, base, entries, assets, b'__rspack_esm_ids=[335708]', 'terminal chunk')
        if b'xiIo(e,t,n)' not in terminal or b'LocalConversationBackgroundTerminalTab' not in terminal:
            raise RuntimeError('terminal data contract changed; manual adaptation required')
        replacements = {composer_name: composer, state_name: state, bootstrap_name: bootstrap}
        for path, entry, _ in entries:
            if path not in replacements:
                continue
            content = replacements[path]
            entry['size'] = len(content)
            integrity = entry.get('integrity')
            if integrity:
                block = integrity['blockSize']
                integrity['hash'] = digest(content)
                integrity['blocks'] = [digest(content[i:i + block]) for i in range(0, len(content), block)]
        offset = 0
        for _, entry, _ in entries:
            entry['offset'] = str(offset)
            offset += entry['size']
        updated_header = json.dumps(header, ensure_ascii=False, separators=(',', ':')).encode('utf-8')
        padding = (-len(updated_header)) % 4
        new_size = 8 + len(updated_header) + padding
        temporary = archive.with_suffix('.candidate')
        binary = executable.read_bytes()
        previous = digest(original_header).encode('ascii')
        if binary.count(previous) != 1:
            raise RuntimeError('executable archive marker is missing or ambiguous')
        modified_binary = binary.replace(previous, digest(updated_header).encode('ascii'), 1)
        with temporary.open('wb') as out:
            out.write(struct.pack('<IIII', first, new_size, new_size - 4, len(updated_header)))
            out.write(updated_header)
            out.write(b'\0' * padding)
            for path, entry, original_offset in entries:
                if path in replacements:
                    out.write(replacements[path])
                    continue
                old.seek(base + original_offset)
                remaining = entry['size']
                while remaining:
                    chunk = old.read(min(8 * 1024 * 1024, remaining))
                    if not chunk:
                        raise RuntimeError(f'truncated archive member: {path}')
                    out.write(chunk)
                    remaining -= len(chunk)
        (app / 'ChatGPT.original.exe').write_bytes(binary)
        executable.write_bytes(modified_binary)
    os.replace(temporary, archive)
    return {'composer': composer_name, 'output_state': state_name, 'bootstrap': bootstrap_name, 'source_header_sha256': previous.decode(), 'patched_header_sha256': digest(updated_header)}


if __name__ == '__main__':
    parser = argparse.ArgumentParser()
    parser.add_argument('--app', type=Path, required=True, help='isolated candidate app directory')
    args = parser.parse_args()
    print(json.dumps(patch(args.app.resolve()), ensure_ascii=False, indent=2))
