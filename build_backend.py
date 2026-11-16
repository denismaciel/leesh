"""Bundle a verified release executable when installing from Git or an sdist."""

from __future__ import annotations

import base64
import csv
import hashlib
import io
import json
import os
import platform
import shutil
import struct
import tempfile
import tomllib
import urllib.request
import zipfile
from pathlib import Path

import uv_build

PLATFORMS = {
    ("Linux", "x86_64"): (
        "x86_64-unknown-linux-musl",
        "manylinux_2_28_x86_64.musllinux_1_2_x86_64",
    ),
    ("Linux", "aarch64"): (
        "aarch64-unknown-linux-musl",
        "manylinux_2_28_aarch64.musllinux_1_2_aarch64",
    ),
    ("Darwin", "x86_64"): ("x86_64-apple-darwin", "macosx_10_13_x86_64"),
    ("Darwin", "arm64"): ("aarch64-apple-darwin", "macosx_11_0_arm64"),
    ("Windows", "AMD64"): ("x86_64-pc-windows-msvc", "win_amd64"),
}


def native_platform() -> tuple[str, str]:
    override = os.environ.get("LEESH_NATIVE_TARGET")
    if override:
        for target, tag in PLATFORMS.values():
            if target == override:
                return target, tag
        raise RuntimeError(f"Unsupported LEESH_NATIVE_TARGET: {override}")
    key = (platform.system(), platform.machine())
    if key not in PLATFORMS:
        raise RuntimeError(f"leesh does not yet ship a native executable for {key}")
    return PLATFORMS[key]


def bundle_binary(root: Path, version: str, target: str) -> Path:
    suffix = ".exe" if target.endswith("windows-msvc") else ""
    destination = root / "src" / "leesh" / "_bin" / f"leesh-rust{suffix}"
    destination.parent.mkdir(parents=True, exist_ok=True)
    override = os.environ.get("LEESH_NATIVE_BINARY")
    if override:
        # Explicit local/CI builds can supply an executable built from this checkout.
        shutil.copyfile(override, destination)
    else:
        manifest = json.loads((root / "native-artifacts.json").read_text())
        if manifest["version"] != version:
            raise RuntimeError("Native artifact manifest does not match the Python package version")
        asset = f"leesh-rust-{target}{suffix}"
        expected = manifest["sha256"][asset]
        url = f"https://github.com/denismaciel/leesh/releases/download/v{version}/{asset}"
        request = urllib.request.Request(url, headers={"User-Agent": "leesh-python-build"})
        with urllib.request.urlopen(request, timeout=120) as response:
            data = response.read()
        if hashlib.sha256(data).hexdigest() != expected:
            raise RuntimeError(f"SHA-256 mismatch for {asset}; refusing to package it")
        destination.write_bytes(data)
    destination.chmod(0o755)
    return destination


def verify_static_linux(binary: Path, target: str) -> None:
    """Validate portable Linux claims from ELF bytes, including cross-arch builds."""
    machines = {"x86_64-unknown-linux-musl": 62, "aarch64-unknown-linux-musl": 183}
    if target not in machines:
        return
    data = binary.read_bytes()
    if len(data) < 64 or data[:7] != b"\x7fELF\x02\x01\x01" or data[7] not in {0, 3}:
        raise RuntimeError("Portable Linux wheels require a valid little-endian ELF64 executable")
    header = struct.unpack_from("<16sHHIQQQIHHHHHH", data)
    _, kind, machine, version, entry, phoff, _, _, ehsize, phsize, phnum, *_ = header
    if (
        kind not in {2, 3}
        or machine != machines[target]
        or version != 1
        or ehsize != 64
        or phsize != 56
        or not 0 < phnum < 65535
        or phoff < 64
        or phoff + phsize * phnum > len(data)
    ):
        raise RuntimeError("ELF executable architecture or program headers do not match target")
    executable_entry = False
    for index in range(phnum):
        ptype, flags, offset, address, _, size, memsize, _ = struct.unpack_from(
            "<IIQQQQQQ", data, phoff + index * phsize
        )
        if offset + size > len(data) or size > memsize:
            raise RuntimeError("Malformed or truncated ELF segment")
        if ptype == 3:  # PT_INTERP: any runtime loader contradicts the static claim.
            raise RuntimeError("Portable Linux wheels require static ELF (PT_INTERP found)")
        if ptype == 1 and flags & 1 and size and address <= entry < address + size:
            executable_entry = True
        if ptype == 2:  # Static PIE may have relocations, but no DT_NEEDED libraries.
            if not size or size % 16:
                raise RuntimeError("Malformed ELF dynamic table")
            terminated = False
            for position in range(offset, offset + size, 16):
                dynamic_tag, _ = struct.unpack_from("<qQ", data, position)
                if dynamic_tag == 1:
                    raise RuntimeError("Portable Linux wheels require static ELF (DT_NEEDED found)")
                if dynamic_tag == 0:
                    terminated = True
                    break
            if not terminated:
                raise RuntimeError("Unterminated ELF dynamic table")
    if not executable_entry:
        raise RuntimeError("ELF executable has no file-backed executable entry point")


def _retag(wheel: Path, tag: str, binary: Path | None = None) -> str:
    """Set native tags and rebuild RECORD after uv_build packages Python sources."""
    with zipfile.ZipFile(wheel) as archive:
        files = {
            info.filename: (info, archive.read(info))
            for info in archive.infolist()
            if not info.is_dir()
        }
    metadata = next(name for name in files if name.endswith(".dist-info/WHEEL"))
    info, data = files[metadata]
    data = data.replace(b"Root-Is-Purelib: true", b"Root-Is-Purelib: false")
    data = b"\n".join(line for line in data.split(b"\n") if not line.startswith(b"Tag:"))
    python_tag, abi_tag, platforms = tag.split("-")
    tags = "".join(f"Tag: {python_tag}-{abi_tag}-{item}\n" for item in platforms.split("."))
    files[metadata] = (info, data.rstrip() + b"\n" + tags.encode())
    if binary is not None:
        name = f"leesh/_bin/{binary.name}"
        info = zipfile.ZipInfo(name, files[metadata][0].date_time)
        info.create_system = 3
        info.compress_type = zipfile.ZIP_DEFLATED
        info.external_attr = 0o100755 << 16
        files[name] = (info, binary.read_bytes())
    record = metadata.removesuffix("WHEEL") + "RECORD"
    rows = []
    for name, (_, data) in sorted(files.items()):
        if name != record:
            digest = base64.urlsafe_b64encode(hashlib.sha256(data).digest()).rstrip(b"=").decode()
            rows.append((name, f"sha256={digest}", str(len(data))))
    output = io.StringIO(newline="")
    csv.writer(output, lineterminator="\n").writerows([*rows, (record, "", "")])
    files[record] = (files[record][0], output.getvalue().encode())
    name = "-".join(wheel.name.split("-")[:-3]) + f"-{tag}.whl"
    destination = wheel.with_name(name)
    with zipfile.ZipFile(destination, "w", compression=zipfile.ZIP_DEFLATED) as archive:
        for info, data in files.values():
            archive.writestr(info, data)
    wheel.unlink()
    return name


def build_wheel(wheel_directory, config_settings=None, metadata_directory=None):
    root = Path.cwd()
    version = tomllib.loads((root / "pyproject.toml").read_text())["project"]["version"]
    target, platform_tag = native_platform()
    # Cross-platform release builds must not replace an editable install's binary.
    with tempfile.TemporaryDirectory(prefix="leesh-wheel-") as temporary:
        staging = Path(temporary)
        shutil.copyfile(root / "native-artifacts.json", staging / "native-artifacts.json")
        binary = bundle_binary(staging, version, target)
        verify_static_linux(binary, target)
        name = uv_build.build_wheel(wheel_directory, config_settings, metadata_directory)
        return _retag(Path(wheel_directory) / name, f"py3-none-{platform_tag}", binary)


def build_editable(wheel_directory, config_settings=None, metadata_directory=None):
    root = Path.cwd()
    version = tomllib.loads((root / "pyproject.toml").read_text())["project"]["version"]
    target, platform_tag = native_platform()
    bundle_binary(root, version, target)
    # Editable development builds may use the host toolchain and dynamic libc.
    if target.endswith("linux-musl"):
        platform_tag = "linux_" + target.removesuffix("-unknown-linux-musl")
    name = uv_build.build_editable(wheel_directory, config_settings, metadata_directory)
    return _retag(Path(wheel_directory) / name, f"py3-none-{platform_tag}")


build_sdist = uv_build.build_sdist
get_requires_for_build_wheel = uv_build.get_requires_for_build_wheel
get_requires_for_build_editable = uv_build.get_requires_for_build_editable
get_requires_for_build_sdist = uv_build.get_requires_for_build_sdist
