#!/usr/bin/env python3
"""Clean all build artifacts for the EBIDownload workspace."""

import shutil
import subprocess
import sys
from pathlib import Path


def main() -> int:
    # The script lives at the project root, so use its directory directly.
    script_dir = Path(__file__).resolve().parent
    project_root = script_dir

    print("🧹 Cleaning Rust workspace...")
    subprocess.run(["cargo", "clean"], cwd=project_root, check=True)

    print("🧹 Cleaning GUI frontend...")
    gui_dir = project_root / "crates" / "ebidownload-gui"

    for name in ("node_modules", "dist"):
        path = gui_dir / name
        if path.exists():
            shutil.rmtree(path)
            print(f"   Removed {path.relative_to(project_root)}")

    tauri_target = gui_dir / "src-tauri" / "target"
    if tauri_target.exists():
        shutil.rmtree(tauri_target)
        print(f"   Removed {tauri_target.relative_to(project_root)}")

    print("✅ All cleaned.")
    print()
    print("To rebuild, run:")
    print("  cd crates/ebidownload-gui && npm install && npm run tauri dev")

    return 0


if __name__ == "__main__":
    try:
        sys.exit(main())
    except subprocess.CalledProcessError as e:
        print(f"❌ Command failed: {e}", file=sys.stderr)
        sys.exit(e.returncode)
    except Exception as e:
        print(f"❌ Error: {e}", file=sys.stderr)
        sys.exit(1)
