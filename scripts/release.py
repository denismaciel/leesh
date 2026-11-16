"""Prepare pinned binaries, verify distributions, and stage immutable release assets."""

from __future__ import annotations

import argparse
import base64
import csv
import hashlib
import html
import io
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import tomllib
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
REPO = "denismaciel/lint-it"
TARGETS = {
    "x86_64-unknown-linux-musl": "manylinux_2_28_x86_64.musllinux_1_2_x86_64",
    "aarch64-unknown-linux-musl": "manylinux_2_28_aarch64.musllinux_1_2_aarch64",
    "x86_64-apple-darwin": "macosx_10_13_x86_64",
    "aarch64-apple-darwin": "macosx_11_0_arm64",
    "x86_64-pc-windows-msvc": "win_amd64",
}


def run(*args: str, cwd: Path = ROOT, env: dict[str, str] | None = None) -> None:
    subprocess.run(args, cwd=cwd, env=env, check=True)


def output(*args: str) -> str:
    return subprocess.check_output(args, cwd=ROOT, text=True).strip()


def version() -> str:
    project = tomllib.loads((ROOT / "pyproject.toml").read_text())["project"]["version"]
    rust = tomllib.loads((ROOT / "rust/Cargo.toml").read_text())["package"]["version"]
    if project != rust:
        raise SystemExit("Python and Rust versions differ")
    if not re.fullmatch(r"\d+\.\d+\.\d+", project):
        raise SystemExit("Expected X.Y.Z shared Python/Cargo version")
    tag = os.environ.get("RELEASE_TAG", f"v{project}")
    if tag and tag != f"v{project}":
        raise SystemExit(f"Tag {tag!r} does not match v{project}")
    return project


def asset(target: str) -> str:
    return f"leesh-rust-{target}" + (".exe" if target.endswith("msvc") else "")


def download_run(run_id: str, destination: Path) -> dict[str, object]:
    if not run_id.isdecimal():
        raise SystemExit("Expected a numeric native workflow run ID")
    info = json.loads(output("gh", "api", f"repos/{REPO}/actions/runs/{run_id}"))
    if (
        info.get("status") != "completed"
        or info.get("conclusion") != "success"
        or info.get("path") != ".github/workflows/native-release.yml"
        or info.get("event") != "workflow_dispatch"
        or info.get("head_branch") != "main"
        or info.get("repository", {}).get("full_name") != REPO
        or info.get("head_repository", {}).get("full_name") != REPO
        or not re.fullmatch(r"[0-9a-f]{40}", info.get("head_sha", ""))
    ):
        raise SystemExit("Expected a successful canonical main workflow_dispatch native run")
    # Fetch the canonical repository explicitly rather than trusting a local remote alias.
    run("git", "fetch", "--no-tags", f"https://github.com/{REPO}.git", "refs/heads/main")
    run("git", "merge-base", "--is-ancestor", info["head_sha"], "FETCH_HEAD")
    inputs = [
        "rust",
        "src",
        "build_backend.py",
        "pyproject.toml",
        "uv.lock",
        "rust-toolchain.toml",
        "scripts/release.py",
        "tests/test_packaging.py",
        ".github/workflows/native-release.yml",
    ]
    if output("git", "status", "--porcelain", "--", *inputs):
        raise SystemExit("Commit build inputs before preparing/staging a release")
    # Only metadata/docs may change between native preparation and the final tag.
    run(
        "git",
        "diff",
        "--exit-code",
        info["head_sha"],
        "HEAD",
        "--",
        *inputs,
    )
    run(
        "gh",
        "run",
        "download",
        run_id,
        "--repo",
        REPO,
        "--pattern",
        "native-*",
        "--dir",
        str(destination),
    )
    return info


def verify_wheel(path: Path, binary: Path | None = None) -> None:
    """Check install metadata, permissions and RECORD for the actual distribution."""
    tag = "-".join(path.stem.split("-")[-3:])
    with zipfile.ZipFile(path) as wheel:
        names = wheel.namelist()
        if len(set(names)) != len(names):
            raise SystemExit("Duplicate wheel members")
        if any(name.startswith("lint_it/") for name in names):
            raise SystemExit("Wheel contains the removed lint_it package")
        entry_points = next(name for name in names if name.endswith(".dist-info/entry_points.txt"))
        if "lint-it =" in wheel.read(entry_points).decode():
            raise SystemExit("Wheel installs the removed lint-it CLI")
        metadata = next(name for name in names if name.endswith(".dist-info/WHEEL"))
        data = wheel.read(metadata).decode()
        python_tag, abi_tag, platforms = tag.split("-")
        expected_tags = {f"Tag: {python_tag}-{abi_tag}-{item}" for item in platforms.split(".")}
        actual_tags = {line for line in data.splitlines() if line.startswith("Tag:")}
        if "Root-Is-Purelib: false" not in data or actual_tags != expected_tags:
            raise SystemExit("Native wheel metadata does not match filename")
        native = [name for name in names if name.startswith("leesh/_bin/")]
        if len(native) != 1 or not wheel.getinfo(native[0]).external_attr >> 16 & 0o111:
            raise SystemExit("Expected one executable bundled binary")
        if binary is not None and wheel.read(native[0]) != binary.read_bytes():
            raise SystemExit("Wheel did not bundle the verified binary")
        record = metadata.removesuffix("WHEEL") + "RECORD"
        rows = list(csv.reader(io.StringIO(wheel.read(record).decode())))
        if {row[0] for row in rows} != set(names):
            raise SystemExit("Wheel RECORD does not cover all members")
        for name, digest, size in rows:
            if name == record:
                if digest or size:
                    raise SystemExit("RECORD must not hash itself")
                continue
            content = wheel.read(name)
            actual = base64.urlsafe_b64encode(hashlib.sha256(content).digest()).rstrip(b"=")
            if digest != f"sha256={actual.decode()}" or size != str(len(content)):
                raise SystemExit(f"Invalid RECORD: {name}")


def prepare(run_id: str) -> None:
    current = version()
    with tempfile.TemporaryDirectory(prefix="leesh-native-") as temporary:
        work = Path(temporary)
        download_run(run_id, work)
        checksums = {}
        for target in TARGETS:
            path = work / f"native-{target}" / asset(target)
            checksums[path.name] = hashlib.sha256(path.read_bytes()).hexdigest()
        manifest = {
            "version": current,
            "rust_tree": output("git", "rev-parse", "HEAD:rust"),
            "workflow_runs": [int(run_id)],
            "sha256": checksums,
        }
        (ROOT / "native-artifacts.json").write_text(json.dumps(manifest, indent=2) + "\n")
    print(
        "Prepared native-artifacts.json. Review and commit before tagging; "
        "binaries are not rebuilt."
    )


def check() -> None:
    current = version()
    with tempfile.TemporaryDirectory(prefix="leesh-release-check-") as temporary:
        work = Path(temporary)
        run("uv", "build", "--no-sources", "--out-dir", str(work / "dist"))
        artifacts = sorted((work / "dist").glob("*.whl")) + sorted((work / "dist").glob("*.tar.gz"))
        if len(artifacts) != 2:
            raise SystemExit("Expected one wheel and one sdist")
        for index, artifact in enumerate(artifacts):
            if artifact.suffix == ".whl":
                verify_wheel(artifact)
            environment = work / f"venv-{index}"
            run("uv", "venv", "--python", sys.executable, str(environment), cwd=work)
            python = environment / ("Scripts/python.exe" if os.name == "nt" else "bin/python")
            run(
                "uv",
                "pip",
                "install",
                "--python",
                str(python),
                "--no-deps",
                str(artifact),
                cwd=work,
            )
            run(
                str(python),
                "-I",
                "-c",
                f"from importlib.metadata import version; assert version('leesh') == {current!r}",
                cwd=work,
            )
            run(str(python), "-I", "-m", "leesh", "rules", cwd=work)
            scripts = environment / ("Scripts" if os.name == "nt" else "bin")
            for command in ("leesh",):
                executable = command + (".exe" if os.name == "nt" else "")
                run(str(scripts / executable), "rules", cwd=work)


def stage() -> None:
    current = version()
    manifest = json.loads((ROOT / "native-artifacts.json").read_text())
    if manifest["version"] != current:
        raise SystemExit("Manifest version differs from package")
    if manifest["rust_tree"] != output("git", "rev-parse", "HEAD:rust"):
        raise SystemExit("Rust sources changed since preparation")
    if len(manifest["workflow_runs"]) != 1:
        raise SystemExit("Expected one complete native build run")
    with tempfile.TemporaryDirectory(prefix="leesh-stage-") as temporary:
        work = Path(temporary)
        download_run(str(manifest["workflow_runs"][0]), work / "native")
        destination = work / "release"
        destination.mkdir()
        for target in TARGETS:
            binary = work / "native" / f"native-{target}" / asset(target)
            digest = hashlib.sha256(binary.read_bytes()).hexdigest()
            if digest != manifest["sha256"][binary.name]:
                raise SystemExit(f"Checksum mismatch: {binary.name}")
            shutil.copy2(binary, destination / binary.name)
            env = {
                **os.environ,
                "LEESH_NATIVE_TARGET": target,
                "LEESH_NATIVE_BINARY": str(binary),
            }
            run("uv", "build", "--wheel", "--no-sources", "--out-dir", str(destination), env=env)
            verify_wheel(next(destination.glob(f"*-py3-none-{TARGETS[target]}.whl")), binary)
        run("uv", "build", "--sdist", "--no-sources", "--out-dir", str(destination))
        checksums, links = [], []
        for path in sorted(destination.iterdir()):
            if path.name.startswith("."):
                continue
            digest = hashlib.sha256(path.read_bytes()).hexdigest()
            checksums.append(f"{digest}  {path.name}\n")
            if path.suffix == ".whl" or path.name.endswith(".tar.gz"):
                url = f"https://github.com/{REPO}/releases/download/v{current}/{path.name}#sha256={digest}"
                links.append(f'<a href="{html.escape(url)}">{html.escape(path.name)}</a><br>')
        (destination / "SHA256SUMS").write_text("".join(checksums))
        (destination / "index.html").write_text(
            "<!doctype html><html><body>\n" + "\n".join(links) + "\n</body></html>\n"
        )
        final = ROOT / "dist/release"
        if final.exists():
            shutil.rmtree(final)
        shutil.copytree(destination, final)


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("command", choices=["prepare", "check", "stage"])
    parser.add_argument("run_id", nargs="?")
    args = parser.parse_args()
    if args.command == "prepare":
        if not args.run_id or not args.run_id.isdecimal():
            parser.error("prepare requires a numeric workflow run ID")
        prepare(args.run_id)
    elif args.command == "stage":
        stage()
    else:
        check()
