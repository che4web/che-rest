#!/usr/bin/env python3
"""Verify unpublished sibling crates and extracted archives without publishing.

Requires Python 3.11+, Git, Cargo and the selected Rust toolchain. Staged
sources, archives and builds stay in a fresh directory outside the repos;
Cargo uses its normal registry and toolchain caches.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import tomllib

PUBLIC_PACKAGES = ("che-orm-macros", "che-orm", "che-sync-protocol", "che-sync", "che-rest")


def manifest(path: Path) -> dict:
    with path.open("rb") as source:
        return tomllib.load(source)


def copy_sources(source: Path, destination: Path) -> None:
    files = subprocess.check_output(
        ["git", "ls-files", "--cached", "--others", "--exclude-standard", "-z"],
        cwd=source,
    )
    for item in files.split(b"\0"):
        if not item:
            continue
        relative = Path(os.fsdecode(item))
        path = source / relative
        if relative.is_absolute() or ".." in relative.parts:
            raise ValueError(f"invalid source path: {relative}")
        if not path.resolve().is_relative_to(source):
            raise ValueError(f"source link escapes repository: {path}")
        if path.is_file():
            target = destination / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(path, target)


def run(command: list[str], cwd: Path, log: Path) -> None:
    print(f"Running {' '.join(command)}\n  log: {log}", flush=True)
    env = os.environ.copy()
    env["CARGO_TARGET_DIR"] = str(cwd / "target")
    with log.open("w") as output:
        result = subprocess.run(command, cwd=cwd, env=env, stdout=output, stderr=subprocess.STDOUT)
    if result.returncode:
        tail = "\n".join(log.read_text(errors="replace").splitlines()[-20:])
        raise RuntimeError(f"command failed ({result.returncode}); see {log}\n{tail}")


def unpack(archive: Path, destination: Path, expected_root: str) -> None:
    with tarfile.open(archive) as source:
        for item in source.getmembers():
            relative = Path(item.name)
            if (
                relative.is_absolute()
                or ".." in relative.parts
                or not relative.parts
                or relative.parts[0] != expected_root
                or not (item.isfile() or item.isdir())
            ):
                raise ValueError(f"unexpected archive entry: {item.name}")
            target = destination / relative
            if item.isdir():
                target.mkdir(parents=True, exist_ok=True)
            else:
                target.parent.mkdir(parents=True, exist_ok=True)
                content = source.extractfile(item)
                if content is None:
                    raise ValueError(f"missing archive entry: {item.name}")
                with content, target.open("wb") as output:
                    shutil.copyfileobj(content, output)
                target.chmod(item.mode & 0o777)
    # Files get fresh timestamps: Cargo must inspect this extraction, even if
    # an archive uses normalized historical timestamps.


def verify(args: argparse.Namespace) -> Path:
    rest = Path(__file__).resolve().parents[1]
    orm = (args.orm if args.orm else rest.parent / "che-orm").resolve()
    if manifest(rest / "Cargo.toml")["package"]["name"] != "che-rest":
        raise ValueError("expected a che-rest checkout")
    orm_data = manifest(orm / "Cargo.toml")
    if orm_data["package"]["name"] != "che-orm":
        raise ValueError("--orm must point to the sibling che-orm workspace")
    workspace = orm_data.get("workspace", {})
    if set(workspace) - {"members", "exclude", "default-members", "resolver"}:
        raise ValueError("review staging before using inherited workspace configuration")
    members = workspace.get("members", [])
    if any("*" in member or ".." in Path(member).parts for member in members):
        raise ValueError("workspace members must be explicit paths within che-orm")

    if args.output:
        root = args.output.resolve()
        if any(root.is_relative_to(repo) for repo in (rest, orm)):
            raise ValueError("--output must be outside both source repositories")
        root.mkdir(parents=True, exist_ok=False)
    else:
        root = Path(tempfile.mkdtemp(prefix="che-sync-packages-"))
    print(f"Verification directory: {root}", flush=True)
    staging = root / "source"
    extracted = root / "archives"
    extracted.mkdir()
    copy_sources(orm, staging / "che-orm")
    copy_sources(rest, staging / "che-rest")
    orm_manifest = staging / "che-orm" / "Cargo.toml"
    text = orm_manifest.read_text()
    text, count = re.subn(r"(?ms)^\[workspace\][^\n]*\n.*?(?=^\[|\Z)", "", text)
    if count != 1:
        raise ValueError("expected one che-orm workspace declaration")
    orm_manifest.write_text(text)
    combined_members = ["che-orm", *(f"che-orm/{member}" for member in members), "che-rest"]
    combined = "[workspace]\nresolver = \"3\"\nmembers = " + json.dumps(combined_members) + "\n"
    if workspace.get("exclude"):
        combined += "exclude = " + json.dumps([f"che-orm/{p}" for p in workspace["exclude"]]) + "\n"
    (staging / "Cargo.toml").write_text(combined)
    command = ["cargo", "package", "--allow-dirty"]
    for package in PUBLIC_PACKAGES:
        command.extend(["-p", package])
    # Resolve a lock for the combined workspace, including unpublished siblings.
    # Offline packaging currently fails in Cargo's temporary-registry handling;
    # only the later extracted-source checks use offline resolution.
    # The source lock files are never modified.
    run(command, staging, root / "package.log")

    paths = {manifest(staging / member / "Cargo.toml")["package"]["name"]: member for member in combined_members}
    checksums = {}
    directories = {}
    for package in PUBLIC_PACKAGES:
        version = manifest(staging / paths[package] / "Cargo.toml")["package"]["version"]
        name = f"{package}-{version}"
        archive = staging / "target" / "package" / f"{name}.crate"
        checksums[archive.name] = hashlib.sha256(archive.read_bytes()).hexdigest()
        directories[package] = name
        unpack(archive, extracted, name)
    normalized = "[workspace]\nresolver = \"3\"\nmembers = " + json.dumps(list(directories.values()))
    normalized += "\n[patch.crates-io]\n"
    for name, directory in directories.items():
        normalized += f"{name} = {{ path = {json.dumps(directory)} }}\n"
    (extracted / "Cargo.toml").write_text(normalized)
    # Packaging resolves each archive's lock; it need not create a workspace
    # lock at the staging root. Seed from the actual REST archive's lock.
    shutil.copy2(extracted / directories["che-rest"] / "Cargo.lock", extracted / "Cargo.lock")
    cargo = ["cargo", f"+{args.toolchain}", "check", "--all-targets"]
    # Offline checks prove that unpublished siblings resolve to extracted
    # archives, with registry dependencies already fetched by packaging.
    run(cargo + ["--workspace", "--features", "che-rest/sync-http", "--offline"], extracted, root / "msrv-sqlite-http.log")
    run(cargo + ["-p", "che-orm", "-p", "che-sync", "--no-default-features", "--features", "postgres", "--offline"], extracted, root / "msrv-postgres.log")
    report = {"result": "passed", "toolchain": args.toolchain, "archives_sha256": checksums}
    (root / "result.json").write_text(json.dumps(report, indent=2) + "\n")
    return root


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--orm", type=Path, help="che-orm checkout; defaults to ../che-orm")
    parser.add_argument("--output", type=Path, help="new directory outside source repositories")
    parser.add_argument("--toolchain", default="1.88.0", help="Rust toolchain used for extracted archives")
    try:
        root = verify(parser.parse_args())
    except (OSError, ValueError, RuntimeError, subprocess.CalledProcessError) as error:
        print(f"Verification failed: {error}", file=sys.stderr)
        return 1
    print(f"All five archives passed. Report: {root / 'result.json'}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
