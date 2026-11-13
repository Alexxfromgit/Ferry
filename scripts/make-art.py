"""Generates Ferry's in-app artwork and installer images from the mascot scenes in branding/art/.

Sources (the mascot in her chair on the dark neon backdrop, lots of empty space around her):
  branding/art/scene-square.png          1:1, mascot bottom-left
  branding/art/scene-portrait.png        9:16, mascot bottom-left
  branding/art/scene-portrait-mirror.png 9:16, mascot bottom-right

Writes:
  app/public/art/scene-portrait.webp    the window backdrop (mirror; she sits bottom-right)
  app/public/art/scene-square.webp      the startup splash and the Settings "About" banner
  app/src-tauri/windows/nsis-sidebar.bmp 164x314, NSIS welcome/finish pages
  app/src-tauri/windows/nsis-header.bmp  150x57, NSIS page header
  app/src-tauri/windows/wix-dialog.bmp   493x312, WiX welcome/exit (art on the left, white for text)
  app/src-tauri/windows/wix-banner.bmp   493x58, WiX page banner (white, app icon on the right)
  app/src-tauri/macos/dmg-background.png 660x400, the .dmg window (icons go on the right)
  app/src-tauri/macos/pkg-background.png the .pkg Installer's left pane (fades out at the top)

Usage (from the repo root; needs Pillow with WebP): python3 scripts/make-art.py
"""

from pathlib import Path

from PIL import Image, ImageChops, ImageDraw, ImageFilter

ROOT = Path(__file__).resolve().parent.parent
ART = ROOT / "branding" / "art"
PUBLIC = ROOT / "app" / "public" / "art"
WINDOWS = ROOT / "app" / "src-tauri" / "windows"
MACOS = ROOT / "app" / "src-tauri" / "macos"
ICON = ROOT / "app" / "public" / "ferry-icon.png"

NIGHT = (8, 6, 11)
MAGENTA = (255, 45, 149)
WHITE = (255, 255, 255)


def load(name: str) -> Image.Image:
    return Image.open(ART / name).convert("RGB")


def cover(img: Image.Image, size: tuple[int, int], focus: tuple[float, float]) -> Image.Image:
    """Scale `img` to fill `size`, cropping around `focus` (fractions of the source)."""
    w, h = size
    scale = max(w / img.width, h / img.height)
    scaled = img.resize((round(img.width * scale), round(img.height * scale)), Image.LANCZOS)
    x = min(max(round(scaled.width * focus[0] - w / 2), 0), scaled.width - w)
    y = min(max(round(scaled.height * focus[1] - h / 2), 0), scaled.height - h)
    return scaled.crop((x, y, x + w, y + h))


def fade_mask(size: tuple[int, int], edge: str, length: int) -> Image.Image:
    """An L mask, opaque except for a linear fade to transparent along one edge."""
    w, h = size
    mask = Image.new("L", size, 255)
    draw = ImageDraw.Draw(mask)
    for i in range(length):
        a = round(255 * i / length)
        if edge == "top":
            draw.line([(0, i), (w, i)], fill=a)
        elif edge == "right":
            draw.line([(w - 1 - i, 0), (w - 1 - i, h)], fill=a)
    return mask


def save_bmp(img: Image.Image, path: Path) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    img.convert("RGB").save(path, "BMP")
    print(f"wrote {path.relative_to(ROOT)}")


def save_png(img: Image.Image, path: Path, dpi: int = 72) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    img.save(path, "PNG", optimize=True, dpi=(dpi, dpi))
    print(f"wrote {path.relative_to(ROOT)}")


def web(img: Image.Image, name: str) -> None:
    PUBLIC.mkdir(parents=True, exist_ok=True)
    path = PUBLIC / name
    img.save(path, "WEBP", quality=80, method=6)
    print(f"wrote {path.relative_to(ROOT)} ({path.stat().st_size // 1024} KB)")


def neon_arrow(canvas: Image.Image, x0: int, x1: int, y: int) -> None:
    """A glowing magenta arrow from x0 to x1, drawn the way the app draws its streaks."""
    glow = Image.new("RGB", canvas.size, (0, 0, 0))
    gd = ImageDraw.Draw(glow)
    core = Image.new("RGB", canvas.size, (0, 0, 0))
    cd = ImageDraw.Draw(core)
    head = [(x1, y), (x1 - 14, y - 10), (x1 - 14, y + 10)]
    for d, width in ((gd, 7), (cd, 2)):
        d.line([(x0, y), (x1 - 8, y)], fill=MAGENTA, width=width)
        d.polygon(head, fill=MAGENTA)
    glow = glow.filter(ImageFilter.GaussianBlur(6))
    out = ImageChops.add(canvas, glow)
    out = ImageChops.lighter(out, core)
    canvas.paste(out)


def main() -> None:
    square = load("scene-square.png")
    portrait = load("scene-portrait.png")
    mirror = load("scene-portrait-mirror.png")

    # In-app artwork, at native size.
    web(mirror, "scene-portrait.webp")
    web(square, "scene-square.webp")

    # Windows: NSIS draws its text beside the sidebar and header images, so they can be dark.
    sidebar = cover(portrait, (164, 314), (0.42, 0.5))
    save_bmp(sidebar, WINDOWS / "nsis-sidebar.bmp")
    # The square scene's empty upper right is all neon streaks.
    streaks = square.crop((480, 150, 1254, 620))
    save_bmp(cover(streaks, (150, 57), (0.6, 0.45)), WINDOWS / "nsis-header.bmp")

    # WiX draws black text over most of its bitmaps, so the art keeps to the left edge.
    dialog = Image.new("RGB", (493, 312), WHITE)
    dialog.paste(cover(portrait, (164, 312), (0.42, 0.5)), (0, 0))
    save_bmp(dialog, WINDOWS / "wix-dialog.bmp")
    banner = Image.new("RGB", (493, 58), WHITE)
    icon = Image.open(ICON).convert("RGBA").resize((44, 44), Image.LANCZOS)
    banner.paste(icon, (493 - 44 - 8, 7), icon)
    save_bmp(banner, WINDOWS / "wix-banner.bmp")

    # macOS .dmg: the mascot on the left, the app and Applications icons on the empty right side
    # (positions in tauri.conf.json), joined by a neon arrow.
    # The streaks are dimmed so the icons and their labels stand out.
    dmg = Image.blend(Image.new("RGB", (660, 400), NIGHT), cover(streaks, (660, 400), (0.5, 0.5)), 0.55)
    her = cover(portrait.crop((0, 700, 941, 1672)), (320, 400), (0.42, 0.5))
    dmg.paste(her, (0, 0), fade_mask(her.size, "right", 140))
    neon_arrow(dmg, 458, 512, 200)
    save_png(dmg, MACOS / "dmg-background.png")

    # macOS .pkg: Installer lists its steps over the top of the left pane, so the art fades out
    # towards the top. Saved at 2x with 144 dpi so it stays sharp on Retina displays.
    w, h = 400, 420
    pkg = cover(portrait, (w, h), (0.42, 0.78)).convert("RGBA")
    pkg.putalpha(fade_mask((w, h), "top", 220))
    save_png(pkg, MACOS / "pkg-background.png", dpi=144)


if __name__ == "__main__":
    main()
