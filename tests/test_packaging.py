"""Verify native artifact selection and integrity without network access."""

from __future__ import annotations

import hashlib
import io
import json
import os
import struct
import subprocess
import zipfile
from pathlib import Path
from unittest.mock import Mock

import pytest

import build_backend
from scripts import release


@pytest.fixture(autouse=True)
def clean_environment(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.delenv("LEESH_NATIVE_TARGET", raising=False)
    monkeypatch.delenv("LEESH_NATIVE_BINARY", raising=False)


@pytest.mark.parametrize(
    ("system", "machine", "target", "tag"),
    [
        (
            "Linux",
            "x86_64",
            "x86_64-unknown-linux-musl",
            "manylinux_2_28_x86_64.musllinux_1_2_x86_64",
        ),
        (
            "Linux",
            "aarch64",
            "aarch64-unknown-linux-musl",
            "manylinux_2_28_aarch64.musllinux_1_2_aarch64",
        ),
        ("Darwin", "x86_64", "x86_64-apple-darwin", "macosx_10_13_x86_64"),
        ("Darwin", "arm64", "aarch64-apple-darwin", "macosx_11_0_arm64"),
        ("Windows", "AMD64", "x86_64-pc-windows-msvc", "win_amd64"),
    ],
)
def test_native_platform(
    monkeypatch: pytest.MonkeyPatch, system: str, machine: str, target: str, tag: str
) -> None:
    monkeypatch.setattr(build_backend.platform, "system", lambda: system)
    monkeypatch.setattr(build_backend.platform, "machine", lambda: machine)
    assert build_backend.native_platform() == (target, tag)


def test_unsupported_platform(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(build_backend.platform, "system", lambda: "Linux")
    monkeypatch.setattr(build_backend.platform, "machine", lambda: "riscv64")
    with pytest.raises(RuntimeError, match=r"does not yet ship.*riscv64"):
        build_backend.native_platform()


def test_explicit_target_overrides_host(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setattr(build_backend.platform, "system", lambda: "Linux")
    monkeypatch.setattr(build_backend.platform, "machine", lambda: "riscv64")
    monkeypatch.setenv("LEESH_NATIVE_TARGET", "aarch64-apple-darwin")
    assert build_backend.native_platform() == ("aarch64-apple-darwin", "macosx_11_0_arm64")


def test_unsupported_explicit_target(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("LEESH_NATIVE_TARGET", "unknown-target")
    with pytest.raises(RuntimeError, match="Unsupported LEESH_NATIVE_TARGET: unknown-target"):
        build_backend.native_platform()


@pytest.mark.parametrize(
    ("target", "filename"),
    [
        ("x86_64-unknown-linux-musl", "leesh-rust"),
        ("x86_64-pc-windows-msvc", "leesh-rust.exe"),
    ],
)
def test_local_binary_override(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, target: str, filename: str
) -> None:
    source = tmp_path / "compiled binary"
    source.write_bytes(b"local executable")
    source.chmod(0o600)
    monkeypatch.setenv("LEESH_NATIVE_BINARY", str(source))
    network = Mock(side_effect=AssertionError("local builds must not download"))
    monkeypatch.setattr(build_backend.urllib.request, "urlopen", network)
    result = build_backend.bundle_binary(tmp_path, "1.2.3", target)
    assert result == tmp_path / "src" / "leesh" / "_bin" / filename
    assert result.read_bytes() == source.read_bytes()
    if os.name != "nt":
        assert result.stat().st_mode & 0o777 == 0o755
    network.assert_not_called()


def write_manifest(root: Path, data: bytes, version: str = "1.2.3") -> None:
    (root / "native-artifacts.json").write_text(
        json.dumps(
            {
                "version": version,
                "sha256": {
                    "leesh-rust-x86_64-unknown-linux-musl": hashlib.sha256(data).hexdigest()
                },
            }
        )
    )


def test_release_download_verifies_checksum(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    data = b"verified release executable"
    write_manifest(tmp_path, data)
    download = Mock(return_value=io.BytesIO(data))
    monkeypatch.setattr(build_backend.urllib.request, "urlopen", download)
    result = build_backend.bundle_binary(tmp_path, "1.2.3", "x86_64-unknown-linux-musl")
    assert result.read_bytes() == data
    if os.name != "nt":
        assert result.stat().st_mode & 0o777 == 0o755
    download.assert_called_once()
    request = download.call_args.args[0]
    assert request.full_url == (
        "https://github.com/denismaciel/leesh/releases/download/v1.2.3/"
        "leesh-rust-x86_64-unknown-linux-musl"
    )
    assert download.call_args.kwargs == {"timeout": 120}


def test_bad_download_preserves_existing_binary(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    write_manifest(tmp_path, b"expected binary")
    existing = tmp_path / "src" / "leesh" / "_bin" / "leesh-rust"
    existing.parent.mkdir(parents=True)
    existing.write_bytes(b"previous binary")
    monkeypatch.setattr(
        build_backend.urllib.request, "urlopen", Mock(return_value=io.BytesIO(b"wrong binary"))
    )
    with pytest.raises(RuntimeError, match="SHA-256 mismatch"):
        build_backend.bundle_binary(tmp_path, "1.2.3", "x86_64-unknown-linux-musl")
    assert existing.read_bytes() == b"previous binary"


def test_manifest_version_must_match_package(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    write_manifest(tmp_path, b"binary", version="1.2.2")
    network = Mock(side_effect=AssertionError("reject stale manifest before downloading"))
    monkeypatch.setattr(build_backend.urllib.request, "urlopen", network)
    with pytest.raises(RuntimeError, match="manifest does not match"):
        build_backend.bundle_binary(tmp_path, "1.2.3", "x86_64-unknown-linux-musl")
    network.assert_not_called()


@pytest.mark.parametrize("target", ["aarch64-apple-darwin", "x86_64-unknown-linux-musl"])
def test_cross_platform_wheel_preserves_editable_binary(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch, target: str
) -> None:
    """Release staging must not replace the executable used by local developers."""
    package = tmp_path / "src/leesh"
    (package / "_bin").mkdir(parents=True)
    (package / "__init__.py").write_text("")
    host_binary = package / "_bin/leesh-rust"
    host_binary.write_bytes(b"host executable")
    target_binary = tmp_path / "macos-executable"
    payload = elf_executable() if "linux" in target else b"macOS executable"
    target_binary.write_bytes(payload)
    (tmp_path / "native-artifacts.json").write_text("{}")
    (tmp_path / "pyproject.toml").write_text(
        '[project]\nname = "leesh"\nversion = "1.2.3"\n'
        '[project.scripts]\nleesh = "leesh._native:main"\n'
        '[build-system]\nrequires = ["uv_build==0.12.4"]\nbuild-backend = "uv_build"\n'
        '[tool.uv.build-backend]\nmodule-name = "leesh"\nsource-exclude = ["_bin/**"]\n'
    )
    monkeypatch.chdir(tmp_path)
    monkeypatch.setenv("LEESH_NATIVE_BINARY", str(target_binary))
    monkeypatch.setenv("LEESH_NATIVE_TARGET", target)
    destination = tmp_path / "dist"
    destination.mkdir()
    filename = build_backend.build_wheel(str(destination))
    assert filename.endswith(f"-py3-none-{release.TARGETS[target]}.whl")
    release.verify_wheel(destination / filename, target_binary)
    assert host_binary.read_bytes() == b"host executable"
    with zipfile.ZipFile(destination / filename) as wheel:
        assert wheel.read("leesh/_bin/leesh-rust") == payload


def elf_executable(
    machine: int = 62, extra_type: int | None = None, dynamic_needed: bool = False
) -> bytes:
    """Small ELF fixture containing executable and optional dynamic segments."""
    count = 1 + (extra_type is not None)
    size = 64 + 56 * count + 32
    ident = b"\x7fELF\x02\x01\x01" + bytes(9)
    data = bytearray(
        struct.pack(
            "<16sHHIQQQIHHHHHH",
            ident,
            3,
            machine,
            1,
            0x400000 + 64 + 56 * count,
            64,
            0,
            0,
            64,
            56,
            count,
            0,
            0,
            0,
        )
    )
    data.extend(struct.pack("<IIQQQQQQ", 1, 5, 0, 0x400000, 0, size, size, 4096))
    if extra_type is not None:
        data.extend(
            struct.pack("<IIQQQQQQ", extra_type, 4, size - 32, 0x400000 + size - 32, 0, 32, 32, 8)
        )
    data.extend(struct.pack("<qQqQ", int(dynamic_needed), 1, 0, 0))
    return bytes(data)


@pytest.mark.parametrize(
    ("machine", "target"), [(62, "x86_64-unknown-linux-musl"), (183, "aarch64-unknown-linux-musl")]
)
@pytest.mark.parametrize("static_pie", [False, True])
def test_static_elf_accepts_both_architectures(tmp_path, machine, target, static_pie):
    binary = tmp_path / "binary"
    binary.write_bytes(elf_executable(machine, 2 if static_pie else None))
    build_backend.verify_static_linux(binary, target)


@pytest.mark.parametrize(
    "data",
    [
        b"not ELF",
        elf_executable()[:100],
        elf_executable(183),
        elf_executable(extra_type=3),
        elf_executable(extra_type=2, dynamic_needed=True),
    ],
)
def test_portable_tag_rejects_unproven_executable(tmp_path, data):
    binary = tmp_path / "binary"
    binary.write_bytes(data)
    with pytest.raises(RuntimeError):
        build_backend.verify_static_linux(binary, "x86_64-unknown-linux-musl")


def native_run_info():
    return {
        "status": "completed",
        "conclusion": "success",
        "event": "workflow_dispatch",
        "path": ".github/workflows/native-release.yml",
        "head_branch": "main",
        "repository": {"full_name": release.REPO},
        "head_repository": {"full_name": release.REPO},
        "head_sha": "a" * 40,
    }


@pytest.mark.parametrize(
    ("field", "value"),
    [
        ("status", "in_progress"),
        ("conclusion", "failure"),
        ("event", "pull_request"),
        ("head_branch", "topic"),
        ("path", ".github/workflows/ci.yml"),
        ("repository", {"full_name": "fork/leesh"}),
        ("head_repository", {"full_name": "fork/leesh"}),
        ("head_sha", "bad"),
    ],
)
def test_native_run_rejects_wrong_provenance(tmp_path, monkeypatch, field, value):
    info = native_run_info()
    info[field] = value
    monkeypatch.setattr(release, "output", Mock(return_value=json.dumps(info)))
    commands = Mock()
    monkeypatch.setattr(release, "run", commands)
    with pytest.raises(SystemExit, match="canonical main"):
        release.download_run("123", tmp_path)
    commands.assert_not_called()


def test_native_run_checks_canonical_main_and_unchanged_inputs(tmp_path, monkeypatch):
    info = native_run_info()
    monkeypatch.setattr(release, "output", Mock(side_effect=[json.dumps(info), ""]))
    commands = Mock()
    monkeypatch.setattr(release, "run", commands)
    assert release.download_run("123", tmp_path) == info
    calls = [call.args for call in commands.call_args_list]
    assert calls[0] == (
        "git",
        "fetch",
        "--no-tags",
        f"https://github.com/{release.REPO}.git",
        "refs/heads/main",
    )
    assert calls[1] == ("git", "merge-base", "--is-ancestor", info["head_sha"], "FETCH_HEAD")
    assert calls[2][:5] == ("git", "diff", "--exit-code", info["head_sha"], "HEAD")
    assert calls[-1][:3] == ("gh", "run", "download")


@pytest.mark.parametrize("failure", ["ancestor", "dirty", "diff"])
def test_native_run_never_downloads_untrusted_inputs(tmp_path, monkeypatch, failure):
    info = native_run_info()
    monkeypatch.setattr(
        release,
        "output",
        Mock(side_effect=[json.dumps(info), "M rust/Cargo.toml" if failure == "dirty" else ""]),
    )
    calls = []

    def command(*args):
        calls.append(args)
        if (failure == "ancestor" and args[:2] == ("git", "merge-base")) or (
            failure == "diff" and args[:2] == ("git", "diff")
        ):
            raise subprocess.CalledProcessError(1, args)

    monkeypatch.setattr(release, "run", command)
    with pytest.raises((SystemExit, subprocess.CalledProcessError)):
        release.download_run("123", tmp_path)
    assert not any(call[:3] == ("gh", "run", "download") for call in calls)
