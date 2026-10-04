#!/usr/bin/env python3
"""Creates a made-up home folder for recording duv's README demo.

Used by assets/demo.tape (see the instructions there), so the recording
never shows anyone's real files and looks the same every time. Prints the
folder's path. Files contain real (zero) data rather than being sparse,
because duv measures allocated space; the whole thing is about 400 MB.
Modification times are set relative to now so the Modified column shows
a spread of ages.

Usage: python3 assets/demo/make_demo.py [folder]
Default folder: /Users/Shared/demo on macOS (no user name in the path),
otherwise /tmp/demo. An existing folder is replaced only if this script
created it (it leaves a marker file next to the folder).
"""

import os
import shutil
import sys
import time

MIB = 1024 * 1024
DAY = 24 * 60 * 60

# (path, size in bytes, age in days)
FILES = [
    ("Videos/vacation-2025.mov", 72 * MIB, 380),
    ("Videos/wedding-highlights.mp4", 48 * MIB, 700),
    ("Videos/screen-recording.mov", 9 * MIB, 3),
    ("Downloads/ubuntu-26.04-desktop-amd64.iso", 64 * MIB, 12),
    ("Downloads/Docker.dmg", 22 * MIB, 40),
    ("Downloads/invoice-0931.pdf", 1 * MIB, 2),
    ("Downloads/old-installer.pkg", 14 * MIB, 500),
    ("backup-2024.tar.gz", 36 * MIB, 290),
    ("notes.md", 12 * 1024, 0),
    ("Documents/thesis-final-FINAL.docx", 3 * MIB, 210),
    ("Documents/taxes-2025.pdf", 2 * MIB, 95),
    ("Projects/game-engine/target/release/engine", 18 * MIB, 1),
    ("Projects/game-engine/src/main.rs", 40 * 1024, 1),
    ("Projects/old-prototype/build/app.bin", 6 * MIB, 900),
]

# (folder, file name pattern, count, size each, age in days)
FILE_SETS = [
    ("Photos/2025", "IMG_{:04d}.heic", 60, 400 * 1024, 120),
    ("Photos/2024", "IMG_{:04d}.heic", 40, 400 * 1024, 480),
    ("Music", "track-{:02d}.flac", 12, 3 * MIB, 600),
    ("Projects/website/node_modules/.pnpm", "pkg-{:03d}.js", 150, 96 * 1024, 30),
    (".cache/thumbnails", "thumb-{:03d}.png", 80, 64 * 1024, 5),
]


def default_folder():
    if sys.platform == "darwin" and os.path.isdir("/Users/Shared"):
        return "/Users/Shared/demo"
    return "/tmp/demo"


def write_file(path, size, age_days, now):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    chunk = b"\0" * MIB
    with open(path, "wb") as out:
        remaining = size
        while remaining > 0:
            part = min(remaining, len(chunk))
            out.write(chunk[:part])
            remaining -= part
    stamp = now - age_days * DAY
    os.utime(path, (stamp, stamp))


def main():
    root = os.path.abspath(sys.argv[1] if len(sys.argv) > 1 else default_folder())
    # Next to the folder rather than inside, so it doesn't show in the demo.
    marker = root.rstrip("/") + ".duv-demo"
    if os.path.exists(root):
        if not os.path.exists(marker):
            sys.exit(f"{root} exists and wasn't made by this script; not touching it")
        shutil.rmtree(root)

    now = time.time()
    for path, size, age in FILES:
        write_file(os.path.join(root, path), size, age, now)
    for folder, pattern, count, size, age in FILE_SETS:
        for i in range(count):
            # Spread ages a little within each set.
            write_file(os.path.join(root, folder, pattern.format(i + 1)), size, age + i % 7, now)

    # Folders take the time of their newest contents, as they would after
    # real use (instead of all showing "just now").
    for folder, subfolders, files in os.walk(root, topdown=False):
        children = [os.path.join(folder, name) for name in subfolders + files]
        if children:
            newest = max(os.path.getmtime(child) for child in children)
            os.utime(folder, (newest, newest))

    with open(marker, "w") as out:
        out.write(f"{root} was created by duv's assets/demo/make_demo.py; both are safe to delete.\n")
    print(root)


if __name__ == "__main__":
    main()
