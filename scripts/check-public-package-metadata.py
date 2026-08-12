#!/usr/bin/env python3
"""Fail when public package metadata drifts from the Carryforth repository."""

from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path


ROOT = Path(__file__).resolve().parent.parent
CANONICAL_REPOSITORY = "https://github.com/lgYanami/Carryforth"
CANONICAL_GIT_REPOSITORY = f"{CANONICAL_REPOSITORY}.git"


def fail(message: str) -> None:
    print(f"metadata error: {message}", file=sys.stderr)
    raise SystemExit(1)


def cargo_metadata(manifest: Path | None = None) -> dict:
    command = [
        "cargo",
        "metadata",
        "--locked",
        "--no-deps",
        "--format-version",
        "1",
    ]
    if manifest is not None:
        command.extend(["--manifest-path", str(manifest)])
    result = subprocess.run(
        command,
        cwd=ROOT,
        check=True,
        stdout=subprocess.PIPE,
        text=True,
    )
    return json.loads(result.stdout)


def check_cargo_package(package: dict) -> None:
    name = package["name"]
    if package.get("source") is not None:
        return
    if package.get("license") != "Apache-2.0":
        fail(f"Cargo package {name} must declare Apache-2.0")
    if package.get("repository") != CANONICAL_REPOSITORY:
        fail(f"Cargo package {name} has a non-Carryforth repository")
    if not package.get("description"):
        fail(f"Cargo package {name} is missing a description")


def check_javascript_package(relative: str) -> None:
    path = ROOT / relative
    package = json.loads(path.read_text())
    repository = package.get("repository")
    repository_url = repository.get("url") if isinstance(repository, dict) else repository
    if package.get("license") != "Apache-2.0":
        fail(f"{relative} must declare Apache-2.0")
    if repository_url != CANONICAL_GIT_REPOSITORY:
        fail(f"{relative} has a non-Carryforth repository")
    if not package.get("description"):
        fail(f"{relative} is missing a description")


def main() -> None:
    for package in cargo_metadata()["packages"]:
        check_cargo_package(package)
    for package in cargo_metadata(ROOT / "desktop/src-tauri/Cargo.toml")["packages"]:
        if Path(package["manifest_path"]).resolve().is_relative_to(ROOT):
            check_cargo_package(package)

    for relative in (
        "package.json",
        "desktop/package.json",
        "web/package.json",
        "admin-web/package.json",
    ):
        check_javascript_package(relative)

    print("Carryforth public package metadata check passed.")


if __name__ == "__main__":
    main()
