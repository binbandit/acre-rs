#!/usr/bin/env python3
"""Build isolated, persistent repositories and time the real Acre shell/CLI flows."""

import argparse
import json
import os
from pathlib import Path
import shutil
import statistics
import subprocess
import tempfile
import threading
import time


def run(argv, cwd, env, **kwargs):
    return subprocess.run(
        argv, cwd=cwd, env=env, check=True, capture_output=True, timeout=120, **kwargs
    )


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def playground(root, binary, args):
    root.mkdir()
    repo = root / "repo"
    repo.mkdir()
    env = {
        key: value for key, value in os.environ.items()
        if not key.startswith(("ACRE_", "GIT_"))
    }
    env.update(GIT_CONFIG_GLOBAL=os.devnull, GIT_CONFIG_NOSYSTEM="1")
    config = root / "config.json"
    write_json(config, {
        "root": str(root / "state"),
        "pool": {"minSlots": args.slots, "maxSlots": args.slots, "replenish": False},
    })
    env["ACRE_CONFIG"] = str(config)

    def git(*argv, **kwargs):
        return run(["git", *argv], repo, env, **kwargs)

    def acre(*argv, cwd=repo, extra=None):
        return run([str(binary), *argv], cwd, env | (extra or {}))

    git("init", "-q", "-b", "main")
    git("config", "user.name", "Acre Performance Playground")
    git("config", "user.email", "playground@example.invalid")
    (repo / ".gitignore").write_text("node_modules/\n.env\n")
    (repo / "package.json").write_text('{"name":"playground","private":true}\n')
    for index in range(args.files):
        path = repo / "packages" / f"pkg-{index % 20:02}" / f"file-{index:05}.js"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(f"export const value = {index};\n" * 10)
    git("add", ".")
    git("commit", "-qm", "Initial playground")
    commits = []
    for index in range(20):
        (repo / "history.txt").write_text(f"Revision {index}\n")
        git("add", "history.txt")
        git("commit", "-qm", f"Playground revision {index}")
        commits.append(git("rev-parse", "HEAD").stdout.decode().strip())
    # Bulk ref creation keeps setup cheap while exercising a large, genuine Git ref set.
    refs = "".join(
        f"create refs/heads/feature/branch-{index:05} {commits[index % len(commits)]}\n"
        for index in range(args.branches)
    )
    git("update-ref", "--stdin", input=refs.encode())
    for index in range(args.cache_files):
        path = repo / "node_modules" / f"dep-{index // 20:04}" / f"file-{index:05}.js"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(f"module.exports = {index};\n" * 40)

    print(f"  Registering {args.worktrees} worktrees...", flush=True)
    for index in range(args.worktrees):
        branch = f"feature/branch-{index:05}"
        git("worktree", "add", "-q", str(root / "external" / str(index)), branch)
        acre(branch, "--json")

    samples = {}
    sequence = 0
    trace_dir = root / "traces"
    trace_dir.mkdir()

    def measure(name, argv, cwd=repo, extra=None):
        nonlocal sequence
        sequence += 1
        trace = trace_dir / f"{sequence:03}-{name}.jsonl"
        started = time.perf_counter()
        output = acre(*argv, cwd=cwd, extra={"GIT_TRACE2_EVENT": str(trace)} | (extra or {}))
        elapsed = (time.perf_counter() - started) * 1000
        events = [json.loads(line) for line in trace.read_text().splitlines()] if trace.exists() else []
        commands = {event["sid"]: event["argv"] for event in events if event["event"] == "start"}
        completed = [
            {"milliseconds": round(event["t_abs"] * 1000, 2), "argv": commands[event["sid"]]}
            for event in events
            if event["event"] == "exit" and event["sid"] in commands
        ]
        samples.setdefault(name, []).append({
            "milliseconds": round(elapsed, 2),
            "git_processes": sum(event.get("event") == "start" for event in events),
            "argv": argv,
            "slowest_git_commands": sorted(completed, key=lambda row: row["milliseconds"], reverse=True)[:5],
        })
        return output

    print("  Timing cold creation, navigation, and warm creation...", flush=True)
    for index in range(args.runs):
        measure("new-cold", ["new", f"bench/cold-{index}", "--stay", "--json"])

    directive = root / "directive"
    shell = {
        "ACRE_SHELL_SESSION_ID": "performance-shell",
        "ACRE_SHELL_PID": str(os.getpid()),
        "ACRE_DIRECTIVE_FILE": str(directive),
    }
    branch = f"feature/branch-{args.worktrees - 1:05}"
    acre(branch, extra=shell)
    destination = Path(os.fsdecode(directive.read_bytes().split(b"\0")[2]))
    cwd = destination
    for _ in range(args.runs * 2):
        measure("previous", ["-"], cwd=cwd, extra=shell)
        cwd = Path(os.fsdecode(directive.read_bytes().split(b"\0")[2]))
    assert cwd == destination
    for index in range(args.runs):
        acre("system", "warm")
        output = measure("new-warm", ["new", f"bench/warm-{index}", "--stay", "--json"])
        assert json.loads(output.stdout)["reused"], "warm creation did not reuse the pool"

    if os.name == "posix":
        import fcntl

        # Deterministic stand-in for a replenisher holding the same lock during cache cloning.
        # The lock is real and is held outside the measured Acre process.
        lock_path = next((root / "state" / "repositories").glob("*/state.lock"))
        with lock_path.open("r+") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            timer = threading.Timer(args.lock_seconds, lambda: fcntl.flock(lock, fcntl.LOCK_UN))
            timer.start()
            try:
                measure("previous-busy", ["-"], cwd=cwd, extra=shell)
            finally:
                timer.join()

    write_json(root / "samples.json", samples)
    # No automatic shell integration or user config changes. This launcher targets only this fixture.
    if os.name == "posix":
        import shlex

        launcher = root / "acre"
        launcher.write_text(
            "#!/bin/sh\n"
            f"export ACRE_CONFIG={shlex.quote(str(config))}\n"
            f"export ACRE_EXECUTABLE={shlex.quote(str(launcher))}\n"
            f"export GIT_CONFIG_GLOBAL={shlex.quote(os.devnull)} GIT_CONFIG_NOSYSTEM=1\n"
            f"exec {shlex.quote(str(binary))} \"$@\"\n"
        )
        launcher.chmod(0o755)
    return {
        name: {
            "median_ms": round(statistics.median(row["milliseconds"] for row in rows), 2),
            "min_ms": min(row["milliseconds"] for row in rows),
            "max_ms": max(row["milliseconds"] for row in rows),
            "median_git_processes": statistics.median(row["git_processes"] for row in rows),
        }
        for name, rows in samples.items()
    }


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/release/acre"))
    parser.add_argument("--compare", type=Path, help="Optional original binary to run first")
    parser.add_argument("--output", type=Path, help="New directory to keep repositories and measurements")
    parser.add_argument("--branches", type=int, default=1000)
    parser.add_argument("--worktrees", type=int, default=24)
    parser.add_argument("--files", type=int, default=2000)
    parser.add_argument("--cache-files", type=int, default=5000)
    parser.add_argument("--slots", type=int, default=8)
    parser.add_argument("--runs", type=int, default=5)
    parser.add_argument("--lock-seconds", type=float, default=2)
    args = parser.parse_args()
    if min(args.branches, args.worktrees, args.files, args.cache_files, args.slots, args.runs) < 1:
        parser.error("counts must be positive")
    if args.worktrees > args.branches or args.lock_seconds <= 0:
        parser.error("worktrees must not exceed branches; lock duration must be positive")
    binaries = {"before": args.compare, "after": args.binary} if args.compare else {"current": args.binary}
    binaries = {name: binary.resolve(strict=True) for name, binary in binaries.items()}
    root = args.output.resolve() if args.output else Path(tempfile.mkdtemp(prefix="acre-playground-")).resolve()
    if args.output:
        root.mkdir(parents=True, exist_ok=False)
    print(f"Playground: {root}", flush=True)
    results = {"parameters": vars(args) | {"binary": str(args.binary), "compare": str(args.compare), "output": str(root)}}
    for label, binary in binaries.items():
        print(f"Running {label}: {binary}", flush=True)
        # Keep each exact executable with its fixture, even if the caller rebuilds later.
        saved = root / f"acre-{label}{binary.suffix if os.name == 'nt' else ''}"
        shutil.copy2(binary, saved)
        results[label] = playground(root / label, saved, args)
        for scenario, metrics in results[label].items():
            print(f"  {scenario:16} {metrics['median_ms']:9.2f} ms  ({metrics['median_git_processes']:g} Git processes)", flush=True)
    write_json(root / "results.json", results)
    print(f"Results and individual Git traces saved in {root}")


if __name__ == "__main__":
    main()
