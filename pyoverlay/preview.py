"""Offline design preview: draw annotations onto a real rumour frame.

Renders the SAME draw code the live overlay uses onto a fixture screenshot
and saves a PNG, so the look can be judged/iterated without the live
(uncapturable) overlay.

  python3 -m pyoverlay.preview app/tests/fixtures/rumours/rumour-3.png out.png
"""
from __future__ import annotations

import sys

import cairo
import numpy as np
from PIL import Image

from .recognize.rumours import RumourRecognizer
from .render import draw_annotations


def preview(frame_path: str, out_path: str) -> None:
    rec = RumourRecognizer()
    pil = Image.open(frame_path).convert("RGB")
    frame = np.asarray(pil)[..., ::-1]           # BGR for recognizer
    hits = rec.recognize(frame)
    print(f"{len(hits)} rumour(s): " +
          ", ".join(f"{h.rumour.name}[{h.rumour.rating}]" for h in hits))

    # Cairo surface from the RGB frame.
    w, h = pil.size
    buf = bytearray(pil.tobytes("raw", "BGRa") if False else b"")
    surf = cairo.ImageSurface(cairo.FORMAT_ARGB32, w, h)
    cr = cairo.Context(surf)
    # Paint the game frame as the background.
    bg = Image.frombytes("RGBA", (w, h), Image.open(frame_path).convert("RGBA").tobytes())
    bg_surf = _pil_to_surface(bg)
    cr.set_source_surface(bg_surf, 0, 0)
    cr.paint()

    draw_annotations(cr, hits, scale=1.0)
    surf.write_to_png(out_path)
    print(f"wrote {out_path}")


def _pil_to_surface(im: Image.Image) -> cairo.ImageSurface:
    im = im.convert("RGBA")
    w, h = im.size
    data = bytearray(im.tobytes("raw", "BGRA"))
    return cairo.ImageSurface.create_for_data(data, cairo.FORMAT_ARGB32, w, h)


if __name__ == "__main__":
    src = sys.argv[1] if len(sys.argv) > 1 else "app/tests/fixtures/rumours/rumour-3.png"
    dst = sys.argv[2] if len(sys.argv) > 2 else "/tmp/rumour-preview.png"
    preview(src, dst)
