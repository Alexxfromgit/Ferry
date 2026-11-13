"""Write the signed updater feed and checksums before publishing a draft release."""

from base64 import b64decode
import hashlib
import json
import os
from pathlib import Path
import sys


def prepare_assets(directory: Path, tag: str, repository: str, public_key: str) -> None:
    version = tag.removeprefix("v")
    base = f"Ferry-{version}"
    installers = [
        "windows-x64.msi",
        "windows-x64.exe",
        "macos-arm64.dmg",
        "macos-x64.dmg",
        "macos-arm64.pkg",
        "macos-x64.pkg",
        "linux-x64.deb",
        "linux-x64.AppImage",
    ]
    targets = {
        "windows-x86_64-msi": "windows-x64.msi",
        "windows-x86_64-nsis": "windows-x64.exe",
        "darwin-aarch64-app": "macos-arm64.app.tar.gz",
        "darwin-x86_64-app": "macos-x64.app.tar.gz",
        "linux-x86_64-appimage": "linux-x64.AppImage",
    }
    key_bytes = b64decode(b64decode(public_key).splitlines()[1])
    if len(key_bytes) != 42:
        raise ValueError("Invalid FERRY_UPDATER_PUBLIC_KEY")

    platforms = {}
    for target, suffix in targets.items():
        filename = f"{base}-{suffix}"
        signature = (directory / f"{filename}.sig").read_text(encoding="utf-8").strip()
        lines = b64decode(signature).decode("utf-8").splitlines()
        signature_bytes = b64decode(lines[1])
        if len(signature_bytes) != 74 or signature_bytes[2:10] != key_bytes[2:10]:
            raise ValueError(f"{filename} was signed with the wrong updater key")
        if f"version:{version}" not in lines[2].split("\t"):
            raise ValueError(f"{filename} signature must be bound to {version}")
        platforms[target] = {
            "signature": signature,
            "url": f"https://github.com/{repository}/releases/download/{tag}/{filename}",
        }

    checksums = []
    for suffix in dict.fromkeys([*installers, *targets.values()]):
        file = directory / f"{base}-{suffix}"
        if file.stat().st_size == 0:
            raise ValueError(f"Empty release asset {file.name}")
        with file.open("rb") as asset:
            digest = hashlib.file_digest(asset, "sha256").hexdigest()
        checksums.append(f"{digest}  {file.name}\n")

    manifest = {"version": version, "platforms": platforms}
    (directory / "latest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    (directory / "SHA256SUMS.txt").write_text("".join(checksums), encoding="utf-8")


if __name__ == "__main__":
    assets, release_tag, repo = sys.argv[1:]
    prepare_assets(Path(assets), release_tag, repo, os.environ["FERRY_UPDATER_PUBLIC_KEY"])
