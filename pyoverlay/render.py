"""Shared annotation drawing (cairo).

The SAME draw_annotations() renders both the live layer-shell overlay and
the offline design preview, so what we approve on a screenshot is exactly
what appears in game. Each rumour's rating badge is anchored just outside
the panel's right edge, aligned to that rumour's line, so it follows the
text without ever covering the game's own UI.
"""
from __future__ import annotations

import gi

gi.require_version("Pango", "1.0")
gi.require_version("PangoCairo", "1.0")
from gi.repository import Pango, PangoCairo  # noqa: E402

# Rating tier -> accent color (RGB 0..1). Palette from
# danielmtv2/poe2-expedition-overlay.
_TIER = {
    "S": (1.00, 0.82, 0.29),   # gold
    "A": (0.37, 0.83, 0.37),   # green
    "B": (0.31, 0.64, 0.94),   # blue
    "C": (0.72, 0.72, 0.72),   # grey
    "D": (0.94, 0.64, 0.31),   # orange
    "F": (0.91, 0.36, 0.36),   # red
    "?": (0.61, 0.50, 0.83),   # purple
}


def tier_color(rating: str):
    r = (rating or "?").strip()
    return _TIER.get(r[:1].upper() if r else "?", (0.72, 0.72, 0.72))


def _layout(cr, s, font):
    lay = PangoCairo.create_layout(cr)
    lay.set_text(s, -1)
    lay.set_font_description(Pango.FontDescription(font))
    return lay, lay.get_pixel_size()


def _rounded(cr, x, y, w, h, r):
    import math
    cr.new_sub_path()
    cr.arc(x + w - r, y + r, r, -math.pi / 2, 0)
    cr.arc(x + w - r, y + h - r, r, 0, math.pi / 2)
    cr.arc(x + r, y + h - r, r, math.pi / 2, math.pi)
    cr.arc(x + r, y + r, r, math.pi, 3 * math.pi / 2)
    cr.close_path()


# Tuned against real 4K frames. Anchor tags a fixed distance right of the
# panel so they line up in a tidy column.
RATING_FONT = "Sans Bold 19"
SUB_FONT = "Sans 16"
TAG_H = 40
CONNECT = 18          # gap between panel edge and tag


def draw_annotation(cr, panel, box, rating, sub, scale=1.0):
    px0, py0, px1, py1 = (v / scale for v in panel)
    _, y0, _, y1 = (v / scale for v in box)
    yc = (y0 + y1) / 2
    col = tier_color(rating)
    rtxt = (rating or "?").strip()

    rlay, (rw, rh) = _layout(cr, rtxt, RATING_FONT)
    slay, (sw, sh) = _layout(cr, sub or "", SUB_FONT)
    chip_w = rw + 20
    body_w = (12 + sw + 14) if sub else 8
    total_w = chip_w + body_w
    x = px1 + CONNECT
    y = yc - TAG_H / 2

    # Connector line from panel edge to the tag, in tier color.
    cr.set_source_rgba(*col, 0.85)
    cr.set_line_width(3)
    cr.move_to(px1 + 2, yc)
    cr.line_to(x, yc)
    cr.stroke()

    # Backing pill with a soft drop shadow.
    _rounded(cr, x + 2, y + 3, total_w, TAG_H, 9)
    cr.set_source_rgba(0, 0, 0, 0.40)
    cr.fill()
    _rounded(cr, x, y, total_w, TAG_H, 9)
    cr.set_source_rgba(0.09, 0.08, 0.07, 0.94)
    cr.fill_preserve()
    cr.set_source_rgba(*col, 0.85)
    cr.set_line_width(2)
    cr.stroke()

    # Rating chip (solid tier color, near-black text).
    _rounded(cr, x + 5, y + 5, chip_w, TAG_H - 10, 6)
    cr.set_source_rgba(*col, 1.0)
    cr.fill()
    cr.set_source_rgba(0.10, 0.09, 0.07, 1.0)
    cr.move_to(x + 5 + (chip_w - rw) / 2, y + (TAG_H - rh) / 2)
    PangoCairo.show_layout(cr, rlay)

    # Caption (map / mods).
    if sub:
        cr.set_source_rgba(0.93, 0.89, 0.81, 1.0)
        cr.move_to(x + chip_w + 12, y + (TAG_H - sh) / 2)
        PangoCairo.show_layout(cr, slay)


def draw_annotations(cr, hits, scale=1.0):
    for h in hits:
        draw_annotation(cr, h.panel, h.box, h.rumour.rating, h.rumour.map_type, scale=scale)
