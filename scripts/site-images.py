"""Turns the PNGs from scripts/site-shots.ps1 into the website's WebP images (full and half width).

    python scripts/site-images.py <folder with the PNGs> [site/img]

Needs Pillow. Only the shots the page uses are converted.
"""

import sys
from pathlib import Path

from PIL import Image

SHOTS = ["list", "peers", "pieces", "speed", "files", "light", "settings", "stats"]


def main() -> None:
    src = Path(sys.argv[1])
    out = Path(sys.argv[2] if len(sys.argv) > 2 else Path(__file__).parent.parent / "site" / "img")
    out.mkdir(parents=True, exist_ok=True)
    for name in SHOTS:
        img = Image.open(src / f"{name}.png").convert("RGB")
        for width, suffix in [(img.width, ""), (img.width // 2, "-sm")]:
            scaled = img if width == img.width else img.resize((width, img.height * width // img.width), Image.LANCZOS)
            path = out / f"{name}{suffix}.webp"
            scaled.save(path, "WEBP", quality=84, method=6)
            print(f"{path} {scaled.width}x{scaled.height} {path.stat().st_size // 1024} KiB")


if __name__ == "__main__":
    main()
