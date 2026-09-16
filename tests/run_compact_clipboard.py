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
    runtime = fixture / "runtime"
    runtime.mkdir(mode=0o700)
    # A private runtime directory, no display and no GTK platform theme keep
    # the check runnable in read-only sandboxes without a desktop session.
    environment = dict(os.environ, QT_QPA_PLATFORM="offscreen", QT_QUICK_BACKEND="software",
                       XDG_RUNTIME_DIR=str(runtime))
    for name in ("WAYLAND_DISPLAY", "DISPLAY", "QT_QPA_PLATFORMTHEME"):
        environment.pop(name, None)
    result = subprocess.run(
        ["quickshell", "--no-color", "--path", str(fixture)],
        env=environment, capture_output=True, text=True, timeout=15,
    )
    output = result.stdout + result.stderr
    print(output, end="")
    if result.returncode or "EXACT_COPY_PASSED (5 originals, CompactDetail and StageProse)" not in output:
        raise SystemExit(1)
