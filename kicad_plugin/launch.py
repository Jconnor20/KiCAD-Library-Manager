"""Toolbar button for the PCB editor: starts KiCad Library Manager on the open board's project."""

import os
import shutil
import subprocess
import sys
import tempfile
import traceback
from datetime import datetime
from pathlib import Path

EXE = "klm.exe" if os.name == "nt" else "klm"
LOG = Path(tempfile.gettempdir()) / "klm-plugin.log"


def log(msg):
    with open(LOG, "a", encoding="utf-8") as f:
        f.write(f"{datetime.now():%Y-%m-%d %H:%M:%S} {msg}\n")


def fail(msg):
    """KiCad hides stderr, so log the error and show it in a message box."""
    log(msg)
    if os.name == "nt":
        import ctypes

        ctypes.windll.user32.MessageBoxW(0, f"{msg}\n\nLog: {LOG}", "KiCad Library Manager", 0x10)
    else:
        print(msg, file=sys.stderr)


def find_klm():
    """klm next to this file, else the path in KLM_EXE, else on PATH."""
    candidates = [Path(__file__).resolve().parent / EXE, os.environ.get("KLM_EXE"), shutil.which("klm")]
    for c in candidates:
        if c and Path(c).is_file():
            return str(c)
    raise FileNotFoundError(f"Could not find {EXE}. Put it next to launch.py or set the KLM_EXE environment variable.")


def project_path():
    """The open board's project folder, or "" when no board is open or the project is unsaved."""
    try:
        from kipy import KiCad

        return KiCad().get_board().document.project.path or ""
    except ImportError:
        raise
    except Exception as e:  # no board open, or KiCad's API refused
        log(f"No project from KiCad ({e!r}); launching without --project")
        return ""


def main():
    exe = find_klm()
    log(f"Found {exe}")
    project = project_path()
    cmd = [exe, "--project", project] if project else [exe]
    log(f"Running {cmd}")
    # Detached, so closing KiCad doesn't take KLM (and its prompts about KiCad being open) down with it.
    flags = 0x00000008 | 0x00000200 if os.name == "nt" else 0  # DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
    subprocess.Popen(cmd, creationflags=flags, close_fds=True)


if __name__ == "__main__":
    try:
        main()
    except Exception as e:
        fail(f"{e}\n\n{traceback.format_exc()}" if not isinstance(e, FileNotFoundError) else str(e))
        sys.exit(1)
