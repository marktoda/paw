#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["resvg-py==0.5.0", "pillow==12.0.0"]
# ///
"""Render the app icon PNGs from the SVGs next to this file.

    uv run docs/design/render-icon.py           # write the PNGs
    uv run docs/design/render-icon.py --check   # fail if a PNG is stale

The default icon has no alpha (App Store rule); the dark one keeps its
transparent background so iOS can draw its own; the tinted one is grey only.
"""
import io
import sys
from pathlib import Path

import resvg_py
from PIL import Image

HERE = Path(__file__).resolve().parent
OUT = HERE.parent.parent / "app/Paw/Assets.xcassets/AppIcon.appiconset"

# (source SVG, PNG in the catalog, Pillow mode)
ICONS = [
    ("icon-paw.svg", "AppIcon.png", "RGB"),
    ("icon-paw-dark.svg", "AppIcon-dark.png", "RGBA"),
    ("icon-paw-mono.svg", "AppIcon-tinted.png", "RGB"),
]


def render(svg: Path, mode: str) -> bytes:
    raw = bytes(resvg_py.svg_to_bytes(svg_path=str(svg), width=1024, height=1024))
    image = Image.open(io.BytesIO(raw)).convert(mode)
    out = io.BytesIO()
    # No metadata, so the bytes depend only on the pixels.
    image.save(out, format="PNG", optimize=True)
    return out.getvalue()


def main() -> int:
    check = "--check" in sys.argv[1:]
    stale = []
    for svg, png, mode in ICONS:
        data = render(HERE / svg, mode)
        target = OUT / png
        if check:
            if not target.exists() or target.read_bytes() != data:
                stale.append(str(target))
        else:
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)
            print(target)
    if stale:
        print("stale, run docs/design/render-icon.py:", *stale, sep="\n  ", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
