"""Toolbar button for the PCB editor: starts KiCad Library Manager on the open board's project."""

import os
import shutil
import subprocess
import sys
from pathlib import Path

from kipy import KiCad

EXE = "klm.exe" if os.name == "nt" else "klm"


def find_klm():
    """klm next to this file, else the path in KLM_EXE, else on PATH."""
    candidates = [Path(__file__).resolve().parent / EXE, os.environ.get("KLM_EXE"), shutil.which("klm")]
    for c in candidates:
        if c and Path(c).is_file():
            return str(c)
    sys.exit(f"Could not find {EXE}. Put it next to launch.py or set the KLM_EXE environment variable.")


def main():
    project = KiCad().get_board().document.project.path
    # Detached, so closing KiCad doesn't take KLM (and its prompts about KiCad being open) down with it.
    flags = 0x00000008 | 0x00000200 if os.name == "nt" else 0  # DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
    subprocess.Popen([find_klm(), "--project", project], creationflags=flags, close_fds=True)


if __name__ == "__main__":
    main()
