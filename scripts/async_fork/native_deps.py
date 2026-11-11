#!/usr/bin/env python3
"""Supply locked libcap development files without changing the runner's runtime."""

import argparse
import base64
import hashlib
import io
import json
import os
import re
import subprocess
import tarfile
import urllib.request
from pathlib import Path


def fetch_verified(url, digest):
    if not url.startswith("https://"):
        raise ValueError("Native dependency downloads require HTTPS")
    with urllib.request.urlopen(url, timeout=30) as response:
        content = response.read()
    if hashlib.sha256(content).digest() != digest:
        raise ValueError(f"Native dependency checksum mismatch: {url}")
    return content


def capability_header(content, strip_prefix):
    # Read one regular file; never unpack archive paths, links or executables.
    with tarfile.open(fileobj=io.BytesIO(content)) as archive:
        member = archive.getmember(f"{strip_prefix}/libcap/include/sys/capability.h")
        if not member.isfile():
            raise ValueError("The locked libcap header must be a regular file")
        return archive.extractfile(member).read()


def prepare(repo, prefix):
    subprocess.run(["pkg-config", "--exists", "openssl"], check=True)
    if subprocess.run(["pkg-config", "--exists", "libcap"]).returncode == 0:
        print("Native development dependencies are already available", flush=True)
        return {}

    # Keep the installed, distribution-maintained runtime. Only its development
    # header and linker name are missing from standard runner images.
    runtime = Path(
        subprocess.check_output(
            ["cc", "-print-file-name=libcap.so.2"], text=True
        ).strip()
    ).resolve()
    if not runtime.is_file():
        raise RuntimeError("The system libcap.so.2 runtime is required")

    module = (repo / "MODULE.bazel").read_text()
    version = re.search(r'bazel_dep\(name = "libcap", version = "([^"]+)"\)', module)
    if version is None:
        raise ValueError("Cannot identify the official locked libcap version")
    url = f"https://bcr.bazel.build/modules/libcap/{version[1]}/source.json"
    lock = json.loads((repo / "MODULE.bazel.lock").read_text())
    metadata = json.loads(
        fetch_verified(url, bytes.fromhex(lock["registryFileHashes"][url]))
    )
    algorithm, digest = metadata["integrity"].split("-", 1)
    if algorithm != "sha256":
        raise ValueError("The locked native dependency must use SHA256")
    print(f"Using official Bazel-locked libcap headers: {version[1]}", flush=True)
    content = fetch_verified(metadata["url"], base64.b64decode(digest, validate=True))
    header = capability_header(content, metadata["strip_prefix"])

    prefix = prefix.resolve()
    if any(character.isspace() for character in str(prefix)):
        raise ValueError("Native staging paths must not contain whitespace")
    include = prefix / "include/sys"
    lib = prefix / "lib"
    pc = lib / "pkgconfig"
    include.mkdir(parents=True, exist_ok=True)
    pc.mkdir(parents=True, exist_ok=True)
    (include / "capability.h").write_bytes(header)
    linker_name = lib / "libcap.so"
    linker_name.unlink(missing_ok=True)
    linker_name.symlink_to(runtime)
    (pc / "libcap.pc").write_text(
        f"prefix={prefix}\n"
        "libdir=${prefix}/lib\nincludedir=${prefix}/include\n\n"
        "Name: libcap\nDescription: Locked headers with the installed system runtime\n"
        f"Version: {metadata['strip_prefix'].removeprefix('libcap-')}\n"
        "Libs: -L${libdir} -lcap\nCflags: -I${includedir}\n"
    )
    previous = os.environ.get("PKG_CONFIG_PATH", "")
    return {"PKG_CONFIG_PATH": str(pc) + (f":{previous}" if previous else "")}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--prefix", required=True, type=Path)
    parser.add_argument("--env-file", required=True, type=Path)
    args = parser.parse_args()
    repo = Path(__file__).resolve().parents[2]
    environment = prepare(repo, args.prefix)
    if any("\n" in value or "\r" in value for value in environment.values()):
        raise ValueError("Native environment values must fit on one line")
    args.env_file.parent.mkdir(parents=True, exist_ok=True)
    args.env_file.write_text(
        "".join(f"{key}={value}\n" for key, value in environment.items())
    )


if __name__ == "__main__":
    main()
