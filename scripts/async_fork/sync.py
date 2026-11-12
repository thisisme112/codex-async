#!/usr/bin/env python3
"""Merge upstream in isolation; publish only a validated, fast-forward candidate."""

import argparse
import json
import os
import re
import subprocess
import tempfile
from pathlib import Path

STATE = ".github/async-fork-state.json"
CONFIG = ".github/async-fork.json"
WORKFLOW = ".github/workflows/async-upstream-sync.yml"
RESOLUTIONS = ".github/async-fork-resolutions.json"
REF_CANDIDATE = "refs/async-sync/candidate"
REF_SOURCE = "refs/async-sync/source"
REQUIRED = {
    "codex-rs/core/src/context/background_execution_instructions.rs": [
        "<background_execution>",
        "wait_for_tasks",
    ],
    "codex-rs/core/src/session/mod.rs": ["BackgroundExecutionInstructions"],
    "codex-rs/core/src/tools/spec_plan.rs": [
        "WaitForTasksHandler",
        "completion_wake_enabled",
    ],
    "codex-rs/core/src/tools/code_mode/completion/command_wait.rs": [
        "wait_for_commands",
        "activity.changed()",
    ],
    "codex-rs/core/tests/async_completion.rs": [
        "exec_completion.rs",
        "code_mode_completion.rs",
    ],
    "codex-rs/core/tests/suite/exec_completion.rs": [
        "event_wait_has_no_intermediate_inference_or_duplicate_wake",
        "completion_capability_reaches_model_instructions_and_tools",
    ],
}


def git(repo, *args, env=None, check=True, input=None):
    result = subprocess.run(
        [
            "git",
            "-c",
            "user.name=github-actions[bot]",
            "-c",
            "user.email=41898282+github-actions[bot]@users.noreply.github.com",
            *args,
        ],
        cwd=repo,
        env=env,
        input=input,
        text=True,
        capture_output=True,
    )
    if check and result.returncode:
        raise RuntimeError(result.stderr.strip() or result.stdout.strip())
    return result


def oid(repo, ref):
    return git(repo, "rev-parse", "--verify", f"{ref}^{{commit}}").stdout.strip()


def config(repo):
    value = json.loads((repo / CONFIG).read_text())
    for key in ["target_branch", "source_branch", "upstream_branch"]:
        git(repo, "check-ref-format", "--branch", value[key])
    if os.environ.get("GITHUB_REPOSITORY", value["repository"]) != value["repository"]:
        raise RuntimeError("Repository identity differs from async-fork.json")
    return value


def features(repo):
    for name, markers in REQUIRED.items():
        path = repo / name
        if not path.is_file() or any(
            marker not in path.read_text() for marker in markers
        ):
            raise RuntimeError(
                f"Async feature contract requires manual adaptation: {name}"
            )


def report(output, value):
    output.mkdir(parents=True, exist_ok=True)
    (output / "metadata.json").write_text(json.dumps(value, indent=2) + "\n")
    lines = [
        "# Codex async upstream synchronization",
        "",
        f"Status: **{value['status']}**",
        "",
    ]
    for key in ["base", "upstream", "candidate", "error"]:
        if key in value:
            lines.append(f"{key}: `{str(value[key])[:2000]}`")
    if value.get("conflicts"):
        lines += ["", "Conflicts (usable branch was not changed):", ""]
        lines += [f"- `{name}`" for name in value["conflicts"][:100]]
    if value.get("resolved_conflicts"):
        lines += ["", "Exact reviewed conflict resolutions applied:", ""]
        lines += [f"- `{name}`" for name in value["resolved_conflicts"]]
    text = "\n".join(lines) + "\n"
    (output / "report.md").write_text(text)
    if path := os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(path, "a") as stream:
            stream.write(text)
    if path := os.environ.get("GITHUB_OUTPUT"):
        with open(path, "a") as stream:
            stream.write(
                f"changed={'true' if value['status'] == 'candidate' else 'false'}\n"
            )
            stream.write(f"base={value.get('base', '')}\n")


def resolve_reviewed_conflicts(repo):
    path = repo / RESOLUTIONS
    if not path.is_file():
        return []
    records = json.loads(path.read_text())
    conflicts = set(
        git(repo, "diff", "--name-only", "--diff-filter=U", "-z").stdout.split("\0")
    )
    resolved = []
    pattern = re.compile(
        r"^<<<<<<<[^\n]*\n(.*?)^=======\n(.*?)^>>>>>>>[^\n]*(?:\n|$)", re.M | re.S
    )
    for record in records:
        name = record["path"]
        if name not in conflicts:
            continue
        # Resolve only the exact reviewed base/ours/upstream versions. Any new
        # upstream change or unexpected conflict still requires manual review.
        versions = [
            git(repo, "rev-parse", f":{stage}:{name}", check=False)
            for stage in (1, 2, 3)
        ]
        expected = [record[key] for key in ("base_blob", "ours_blob", "theirs_blob")]
        if any(version.returncode for version in versions) or [
            version.stdout.strip() for version in versions
        ] != expected:
            continue
        source = (repo / name).read_text()
        matches = list(pattern.finditer(source))
        if len(matches) != 1:
            continue
        match = matches[0]
        if (match[1], match[2]) != (record["ours"], record["theirs"]):
            continue
        merged = source[:match.start()] + record["resolved"] + source[match.end():]
        if pattern.search(merged):
            continue
        (repo / name).write_text(merged)
        git(repo, "add", "--", name)
        resolved.append(name)
    return resolved


def prepare(repo, output, upstream_ref=None):
    settings = config(repo)
    if git(repo, "status", "--porcelain").stdout:
        raise RuntimeError("Prepare requires a clean checkout")
    base = oid(repo, "HEAD")
    if upstream_ref is None:
        git(
            repo,
            "fetch",
            "--no-tags",
            settings["upstream_url"],
            f"refs/heads/{settings['upstream_branch']}:refs/remotes/async-upstream/main",
        )
        upstream_ref = "refs/remotes/async-upstream/main"
    upstream = oid(repo, upstream_ref)
    value = {
        "status": "preparing",
        "base": base,
        "upstream": upstream,
        "repository": settings["repository"],
    }
    state = json.loads((repo / STATE).read_text()) if (repo / STATE).exists() else {}
    if state.get("upstream") == upstream:
        value["status"] = "up_to_date"
        report(output, value)
        return value
    with tempfile.TemporaryDirectory(prefix="codex-upstream-") as temporary:
        index_env = dict(os.environ, GIT_INDEX_FILE=str(Path(temporary) / "index"))
        git(repo, "read-tree", upstream, env=index_env)
        # Only upstream workflow entrypoints are filtered. All source and local
        # actions remain upstream-owned. Our sole workflow stays under our control.
        entries = git(
            repo, "ls-files", "-z", "--", ".github/workflows", env=index_env
        ).stdout.split("\0")
        entrypoints = [
            name
            for name in entries
            if name
            and Path(name).parent.as_posix() == ".github/workflows"
            and Path(name).suffix in {".yml", ".yaml"}
        ]
        if entrypoints:
            git(repo, "rm", "--cached", "-f", "--", *entrypoints, env=index_env)
        tree = git(repo, "write-tree", env=index_env).stdout.strip()
        parents = ["-p", upstream]
        if previous := state.get("source"):
            parents = ["-p", oid(repo, previous), *parents]
        source = git(
            repo,
            "commit-tree",
            tree,
            *parents,
            input=f"Official source {upstream}; exclude inherited workflow entrypoints\n",
        ).stdout.strip()
        worktree = Path(temporary) / "candidate"
        git(repo, "worktree", "add", "--detach", str(worktree), base)
        try:
            merge = git(
                worktree, "-c", "merge.conflictStyle=merge", "merge", "--no-commit", "--no-ff", source, check=False
            )
            if merge.returncode:
                resolved = resolve_reviewed_conflicts(worktree)
                if resolved:
                    value["resolved_conflicts"] = resolved
                conflicts = git(
                    worktree, "diff", "--name-only", "--diff-filter=U", "-z"
                ).stdout
                remaining = [name for name in conflicts.split("\0") if name]
                if remaining or not resolved:
                    value.update(status="conflict", conflicts=remaining, error=merge.stderr[-2000:])
                    report(output, value)
                    return value
            changed = git(
                worktree, "diff", "--cached", "--name-only", "-z", base
            ).stdout.split("\0")
            protected = [
                name
                for name in changed
                if name
                and (
                    name.startswith("scripts/async_fork/") or name in [CONFIG, WORKFLOW, RESOLUTIONS]
                )
            ]
            if protected:
                raise RuntimeError(
                    f"Upstream changed synchronization control files: {protected}"
                )
            features(worktree)
            (worktree / STATE).write_text(
                json.dumps({"upstream": upstream, "source": source}, indent=2) + "\n"
            )
            git(worktree, "add", STATE)
            git(
                worktree,
                "commit",
                "-m",
                f"Merge official Codex {upstream[:12]} with async features",
            )
            candidate = oid(worktree, "HEAD")
            git(repo, "update-ref", REF_CANDIDATE, candidate)
            git(repo, "update-ref", REF_SOURCE, source)
            output.mkdir(parents=True, exist_ok=True)
            git(
                repo,
                "bundle",
                "create",
                str(output / "candidate.bundle"),
                REF_CANDIDATE,
                REF_SOURCE,
                f"^{base}",
            )
            value.update(status="candidate", candidate=candidate, source=source)
            report(output, value)
            return value
        finally:
            # This is a private temporary worktree created above; never touch the
            # caller's working tree or abort a merge in the caller's checkout.
            git(repo, "worktree", "remove", "--force", str(worktree))


def promote(repo, output):
    settings = config(repo)
    value = json.loads((output / "metadata.json").read_text())
    if (
        value.get("status") != "candidate"
        or value.get("repository") != settings["repository"]
    ):
        raise RuntimeError("Not an eligible candidate for this repository")
    for key in ["base", "candidate", "source", "upstream"]:
        if not re.fullmatch(r"[0-9a-f]{40}", value[key]):
            raise RuntimeError("Invalid candidate identity")
    git(
        repo,
        "fetch",
        str(output / "candidate.bundle"),
        f"{REF_CANDIDATE}:{REF_CANDIDATE}",
        f"{REF_SOURCE}:{REF_SOURCE}",
    )
    if (
        oid(repo, REF_CANDIDATE) != value["candidate"]
        or oid(repo, REF_SOURCE) != value["source"]
    ):
        raise RuntimeError("Bundle identity mismatch")
    if git(
        repo,
        "merge-base",
        "--is-ancestor",
        value["base"],
        value["candidate"],
        check=False,
    ).returncode:
        raise RuntimeError("Candidate is not a fast-forward of its recorded base")
    target = f"refs/heads/{settings['target_branch']}"
    current = git(repo, "ls-remote", "origin", target).stdout.split()
    if not current or current[0] != value["base"]:
        raise RuntimeError(
            "Usable branch changed during validation; refusing stale promotion"
        )
    # No force push. Atomic publication means either both refs advance or neither.
    git(
        repo,
        "push",
        "--atomic",
        "origin",
        f"{REF_CANDIDATE}:{target}",
        f"{REF_SOURCE}:refs/heads/{settings['source_branch']}",
    )
    value["status"] = "promoted"
    report(output, value)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=["prepare", "promote", "check"])
    parser.add_argument("--output", type=Path)
    parser.add_argument(
        "--upstream-ref", help="Use an already fetched ref (offline inspection/testing)"
    )
    args = parser.parse_args()
    repo = Path(__file__).resolve().parents[2]
    if args.operation == "check":
        features(repo)
        print("Async feature contract present")
        return
    if args.output is None:
        parser.error("--output is required")
    output = args.output.resolve()
    try:
        if args.operation == "prepare":
            value = prepare(repo, output, args.upstream_ref)
            if value["status"] == "conflict":
                print("Manual upstream merge required: " + ", ".join(value.get("conflicts", [])), flush=True)
                raise SystemExit(2)
        else:
            promote(repo, output)
    except (RuntimeError, OSError, ValueError) as error:
        report(output, {"status": "error", "error": str(error)})
        raise SystemExit(1) from error


if __name__ == "__main__":
    main()
