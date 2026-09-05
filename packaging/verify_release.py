"""Validate the four release archives, bundled instructions, and SHA-256 files."""

import hashlib
from pathlib import Path
import sys
import tarfile
import zipfile


def verify(directory: Path, tag: str) -> None:
    expected = set()
    for platform in ("macos-arm64", "macos-x86_64", "windows-x86_64", "linux-x86_64"):
        suffix = ".zip" if platform.startswith("windows") else ".tar.gz"
        name = f"fastcutvid-{tag}-{platform}{suffix}"
        expected.update((name, name + ".sha256"))
        archive = directory / name
        checksum = (directory / (name + ".sha256")).read_text().split()
        assert len(checksum) == 2 and checksum[1] == name, f"Bad checksum filename: {name}"
        assert hashlib.sha256(archive.read_bytes()).hexdigest() == checksum[0], f"Checksum mismatch: {name}"
        if suffix == ".zip":
            with zipfile.ZipFile(archive) as bundle:
                assert bundle.testzip() is None, f"Corrupt ZIP: {name}"
                names = set(bundle.namelist())
        else:
            with tarfile.open(archive) as bundle:
                names = set(bundle.getnames())
        if platform.startswith("macos"):
            root = "fastCutVid.app/Contents/Resources/"
            assert "fastCutVid.app/Contents/MacOS/fast-cutvid" in names
            assert root + "fastCutVid.icns" in names
            assert "fastCutVid.app/Contents/Info.plist" in names
        else:
            root = "" if suffix == ".zip" else f"fastcutvid-{tag}-{platform}/"
            executable = "fast-cutvid.exe" if suffix == ".zip" else "fast-cutvid"
            assert root + executable in names
        for required in (
            "README.md", "CHANGELOG.md", "LICENSE", "AGENTS.md", "CLAUDE.md",
            f"docs/releases/{tag}.md",
            "docs/LOCAL_AGENTS.md", "docs/AGENT_GUIDE.md", "docs/OPERATOR_GUIDE.md",
            "docs/timeline.schema.json", "assets/fastcutvid-logo-readme.png",
            ".agents/skills/fast-cut-timelines/SKILL.md",
            ".agents/skills/fast-cut-timelines/references/timeline-format.md",
        ):
            assert root + required in names, f"Missing {required} in {name}"
        print(f"Verified {name}")
    assert {path.name for path in directory.iterdir()} == expected, "Unexpected or missing release files"


if __name__ == "__main__":
    verify(Path(sys.argv[1]), sys.argv[2])
