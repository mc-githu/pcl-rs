"""Generate the PCL-RS icon set (blue rounded square + white pickaxe glyph).

Run with any Python that has Pillow:  python scripts/make_icons.py
Outputs into src-tauri/icons/.
"""
from pathlib import Path

from PIL import Image, ImageDraw

OUT = Path(__file__).resolve().parent.parent / "src-tauri" / "icons"
BLUE = (34, 119, 221, 255)
BLUE_DARK = (26, 95, 180, 255)
WHITE = (255, 255, 255, 255)

SIZE = 1024


def draw_icon() -> Image.Image:
    img = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))

    # Rounded blue background with a subtle vertical gradient.
    grad = Image.new("RGBA", (SIZE, SIZE), (0, 0, 0, 0))
    gd = ImageDraw.Draw(grad)
    for y in range(SIZE):
        t = y / SIZE
        color = tuple(int(BLUE[i] * (1 - t) + BLUE_DARK[i] * t) for i in range(3)) + (255,)
        gd.line([(0, y), (SIZE, y)], fill=color)

    mask = Image.new("L", (SIZE, SIZE), 0)
    ImageDraw.Draw(mask).rounded_rectangle([0, 0, SIZE - 1, SIZE - 1], radius=200, fill=255)
    img.paste(grad, (0, 0), mask)

    # Pickaxe: handle + head arc.
    d = ImageDraw.Draw(img)
    d.line([(SIZE * 0.40, SIZE * 0.80), (SIZE * 0.66, SIZE * 0.34)], fill=WHITE, width=64)
    d.arc(
        [SIZE * 0.18, SIZE * 0.14, SIZE * 0.86, SIZE * 0.62],
        start=200,
        end=340,
        fill=WHITE,
        width=76,
    )
    return img


def main() -> None:
    OUT.mkdir(parents=True, exist_ok=True)
    icon = draw_icon()
    icon.resize((1024, 1024), Image.LANCZOS).save(OUT / "icon.png")
    icon.resize((128, 128), Image.LANCZOS).save(OUT / "128x128.png")
    icon.resize((32, 32), Image.LANCZOS).save(OUT / "32x32.png")
    icon.resize((256, 256), Image.LANCZOS).save(
        OUT / "icon.ico",
        sizes=[(16, 16), (32, 32), (48, 48), (64, 64), (128, 128), (256, 256)],
    )
    print(f"icons written to {OUT}")


if __name__ == "__main__":
    main()
