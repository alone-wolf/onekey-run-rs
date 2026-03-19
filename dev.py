#!/usr/bin/env python3

from __future__ import annotations

import argparse
import os
import re
import shutil
import stat
import subprocess
import sys
from pathlib import Path

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover
    tomllib = None


REPO_ROOT = Path(__file__).resolve().parent
CARGO_TOML = REPO_ROOT / "Cargo.toml"


def main() -> int:
    parser = build_parser()
    args = parser.parse_args()
    return args.func(args)


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Development helper script for the onekey-run-rs project."
    )
    subparsers = parser.add_subparsers(dest="command", required=True)

    install_parser = subparsers.add_parser(
        "install",
        help="Build the project and install the compiled binary into ~/bin",
    )
    profile_group = install_parser.add_mutually_exclusive_group()
    profile_group.add_argument(
        "--release",
        action="store_true",
        help="Build with the release profile (default)",
    )
    profile_group.add_argument(
        "--debug",
        action="store_true",
        help="Build with the debug profile",
    )
    install_parser.set_defaults(func=run_install)

    return parser


def run_install(args: argparse.Namespace) -> int:
    binary_name = read_binary_name(CARGO_TOML)
    profile = "debug" if args.debug else "release"

    build_cmd = ["cargo", "build"]
    if profile == "release":
        build_cmd.append("--release")

    print(f"==> building {binary_name} with cargo ({profile})")
    subprocess.run(build_cmd, cwd=REPO_ROOT, check=True)

    source_path = built_binary_path(binary_name, profile)
    if not source_path.exists():
        raise SystemExit(f"built binary not found: {source_path}")

    install_dir = Path.home() / "bin"
    install_dir.mkdir(parents=True, exist_ok=True)

    target_path = install_dir / source_path.name
    shutil.copy2(source_path, target_path)
    make_user_executable(target_path)

    print(f"==> installed {source_path.name} to {target_path}")
    return 0


def read_binary_name(cargo_toml_path: Path) -> str:
    raw = cargo_toml_path.read_text(encoding="utf-8")

    if tomllib is not None:
        cargo_data = tomllib.loads(raw)
        package_name = cargo_data.get("package", {}).get("name")
        if package_name:
            return str(package_name)

    match = re.search(r'^\s*name\s*=\s*"([^"]+)"\s*$', raw, flags=re.MULTILINE)
    if match:
        return match.group(1)

    raise SystemExit(f"failed to read package name from {cargo_toml_path}")


def built_binary_path(binary_name: str, profile: str) -> Path:
    filename = binary_name
    if os.name == "nt":
        filename += ".exe"
    return REPO_ROOT / "target" / profile / filename


def make_user_executable(path: Path) -> None:
    if os.name == "nt":
        return

    current_mode = path.stat().st_mode
    path.chmod(
        current_mode
        | stat.S_IXUSR
        | stat.S_IRUSR
        | stat.S_IWUSR
        | stat.S_IXGRP
        | stat.S_IXOTH
    )


if __name__ == "__main__":
    sys.exit(main())
