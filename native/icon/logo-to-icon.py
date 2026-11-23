# Makes String's icon from its logo (native\icon\logo.png, the logo by awepollyon):
#   python native\icon\logo-to-icon.py
# writes native\string-app\assets\String.ico (every size Windows asks for, 16 to 256 px) and String.png (256 px, the
# window's icon). Each size is shrunk from the full logo on its own; the small ones get a little sharpening so the S
# stays crisp on the taskbar.
from pathlib import Path

from PIL import Image, ImageFilter

HERE = Path(__file__).resolve().parent
ASSETS = HERE.parent / "string-app" / "assets"
SIZES = [16, 20, 24, 32, 40, 48, 64, 96, 128, 256]


def shrink(logo: Image.Image, size: int) -> Image.Image:
    im = logo.resize((size, size), Image.LANCZOS)
    if size <= 48:
        im = im.filter(ImageFilter.UnsharpMask(radius=0.6, percent=60, threshold=2))
    return im


def main() -> None:
    logo = Image.open(HERE / "logo.png").convert("RGBA")
    images = [shrink(logo, s) for s in SIZES]
    ASSETS.mkdir(parents=True, exist_ok=True)
    images[-1].save(ASSETS / "String.png")
    images[-1].save(ASSETS / "String.ico", sizes=[(s, s) for s in SIZES], append_images=images[:-1])
    print("wrote", ASSETS / "String.ico", "and String.png")


if __name__ == "__main__":
    main()
