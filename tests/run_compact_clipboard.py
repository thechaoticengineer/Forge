"""Check exact clipboard data in an isolated, offscreen Quickshell process."""
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

repo = Path(__file__).resolve().parents[1]
with tempfile.TemporaryDirectory(prefix="forge-compact-clipboard-") as directory:
    fixture = Path(directory)
    (fixture / "components").mkdir()
    for name in ("CompactDetail.qml", "StageProse.qml", "DetailText.js"):
        shutil.copyfile(repo / "quickshell" / name, fixture / "components" / name)
    shutil.copyfile(repo / "tests/qml/clipboard.qml", fixture / "shell.qml")
    environment = dict(os.environ, QT_QPA_PLATFORM="offscreen", QT_QUICK_BACKEND="software")
    environment.pop("WAYLAND_DISPLAY", None)
    result = subprocess.run(
        ["quickshell", "--no-color", "--path", str(fixture)],
        env=environment, capture_output=True, text=True, timeout=15,
    )
    output = result.stdout + result.stderr
    print(output, end="")
    if result.returncode or "EXACT_COPY_PASSED (5 originals, CompactDetail and StageProse)" not in output:
        raise SystemExit(1)
