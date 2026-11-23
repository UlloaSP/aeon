"""Renders packaging/windows/aeon.ico from the Aeon mark.

The mark: a ring with a block cursor across its right edge, on a dark rounded
tile. Small sizes use heavier strokes and a larger mark so they stay legible,
as the brand sheet specifies. Run it only when the mark changes:

    python scripts/make-icon.py

Needs Pillow. The SVG twin of the 256 px tile is packaging/windows/aeon.svg.
"""

from pathlib import Path

from PIL import Image, ImageDraw

TILE = (0x0C, 0x0D, 0x10, 255)
RING = (0xF2, 0xF1, 0xEC, 255)
CURSOR = (0xFF, 0xB5, 0x47, 255)
SIZES = [16, 20, 24, 32, 40, 48, 64, 128, 256]
# Drawn this many times larger, then downsampled, for clean anti-aliasing.
SUPERSAMPLE = 16


def tile(size: int) -> Image.Image:
    big = size * SUPERSAMPLE
    image = Image.new("RGBA", (big, big), (0, 0, 0, 0))
    draw = ImageDraw.Draw(image)
    draw.rounded_rectangle(
        [0, 0, big - 1, big - 1], radius=round(size * 0.225) * SUPERSAMPLE, fill=TILE
    )

    small = size <= 32
    stroke = 18 if size <= 16 else (16 if small else 14)
    mark = size * (0.84 if small else 0.64) * SUPERSAMPLE
    unit = mark / 120  # the mark's own 120x120 grid
    origin = (big - mark) / 2

    def at(v: float) -> float:
        return origin + v * unit

    # Stroke centered on r = 41 - stroke/2, so the outer edge is always 41.
    draw.ellipse(
        [at(60 - 41), at(60 - 41), at(60 + 41), at(60 + 41)],
        outline=RING,
        width=round(stroke * unit),
    )
    draw.rectangle([at(101 - stroke), at(19), at(101), at(101)], fill=CURSOR)
    return image.resize((size, size), Image.LANCZOS)


def main() -> None:
    out = Path(__file__).resolve().parent.parent / "packaging" / "windows" / "aeon.ico"
    frames = [tile(s) for s in SIZES]
    frames[-1].save(
        out,
        format="ICO",
        sizes=[(s, s) for s in SIZES],
        append_images=frames[:-1],
    )
    print(f"wrote {out} ({out.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
