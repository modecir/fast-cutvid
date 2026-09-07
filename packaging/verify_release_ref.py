"""Accept final version tags only on main's release history."""

import re
import subprocess
import sys


def verify(tag: str) -> None:
    if not re.fullmatch(r"v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)", tag):
        raise ValueError("Release tags must be final versions in the form vX.Y.Z")
    commit = subprocess.check_output(
        ["git", "rev-parse", "--verify", f"refs/tags/{tag}^{{commit}}"], text=True
    ).strip()
    releases = subprocess.check_output(
        ["git", "rev-list", "--first-parent", "refs/remotes/origin/main"], text=True
    ).splitlines()
    if commit not in releases:
        raise ValueError(f"{tag} must point to a release commit on main")


if __name__ == "__main__":
    if len(sys.argv) != 2:
        sys.exit("Usage: verify_release_ref.py vX.Y.Z")
    try:
        verify(sys.argv[1])
    except (ValueError, subprocess.CalledProcessError) as error:
        sys.exit(str(error))
    print(f"{sys.argv[1]} is a final version on main")
