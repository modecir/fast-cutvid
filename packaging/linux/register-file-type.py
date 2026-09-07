#!/usr/bin/env python3
"""Register this extracted fastCutVid release for the current desktop user."""
import os
from pathlib import Path
import subprocess

bundle = Path(__file__).resolve().parent
binary = bundle / "fast-cutvid"
if not binary.is_file():
    raise SystemExit("Keep this script beside the fast-cutvid executable before running it.")
data = Path(os.environ.get("XDG_DATA_HOME", str(Path.home() / ".local/share")))
applications = data / "applications"
packages = data / "mime/packages"
applications.mkdir(parents=True, exist_ok=True)
packages.mkdir(parents=True, exist_ok=True)
# Desktop Exec quoting is distinct from shell quoting. Escape reserved characters
# and literal percent signs before appending the single-file placeholder.
executable = str(binary).replace("\\", "\\\\").replace('"', '\\"').replace('`', '\\`').replace('$', '\\$').replace('%', '%%')
# Desktop string-value escaping is applied before Exec argument parsing.
executable = executable.replace("\\", "\\\\")
(applications / "com.modecir.fastcut.desktop").write_text(
    '[Desktop Entry]\nType=Application\nName=fastCutVid\n'
    f'Exec="{executable}" %f\nTerminal=false\n'
    'MimeType=application/x-fastcutvid-project;\nCategories=AudioVideo;Video;\n'
)
(packages / "com.modecir.fastcut.xml").write_text('''<?xml version="1.0" encoding="UTF-8"?>
<mime-info xmlns="http://www.freedesktop.org/standards/shared-mime-info">
  <mime-type type="application/x-fastcutvid-project">
    <comment>fastCutVid Project</comment>
    <sub-class-of type="application/json"/>
    <glob pattern="*.fastcut"/>
  </mime-type>
</mime-info>
''')
subprocess.run(["update-mime-database", str(data / "mime")], check=True)
subprocess.run(["update-desktop-database", str(applications)], check=True)
subprocess.run(["xdg-mime", "default", "com.modecir.fastcut.desktop", "application/x-fastcutvid-project"], check=True)
print("Registered .fastcut projects. Run this script again if you move the release folder.")
