"""Expedition Island Rumour recognizer.

Mimics danielmtv2/poe2-expedition-overlay:
  1. Cheap downscaled full-frame OCR locates the tooltip by its anchors
     ("UNCHARTED WATERS" top, "REQUIRES"/"CONSUMES" bottom). Idle frames
     (no tooltip) cost only this pre-scan.
  2. Crop the rumour region between the anchors, within the tooltip column.
  3. Re-OCR the crop at several scales and union the matches (tesseract
     groups/drops lines differently per scale, so a line missed at one is
     caught at another; each still passes the strict threshold, so union
     raises recall without false positives).
  4. Fuzzy-match each line to the known rumours (matching.RumourMatcher).
"""
from __future__ import annotations

from dataclasses import dataclass
from pathlib import Path

import numpy as np
from rapidfuzz import fuzz
from scipy import ndimage

from ..matching import Rumour, RumourMatcher, load_rumours, _cnorm
from ..ocr import Line, ocr_lines

ANCHOR_TOP = "UNCHARTED WATERS"
ANCHOR_BOTTOM = ("CONSUMES", "REQUIRES")
DETECT_SCALE = 0.5          # cheap anchor pre-scan on the full frame
CROP_SCALES = (1.0, 1.5, 2.0)
ANCHOR_MIN = 60.0           # anchor phrase fuzzy score to accept a tooltip

_DATA = Path(__file__).resolve().parent.parent / "data" / "rumours.csv"


@dataclass
class RumourHit:
    rumour: Rumour
    box: tuple[int, int, int, int]   # screen box of the matched line
    raw: str
    panel: tuple[int, int, int, int] = (0, 0, 0, 0)  # rumour-region box on screen


def _to_gray(frame: np.ndarray) -> np.ndarray:
    if frame.ndim == 2:
        return frame
    # BGR or RGB -> luma; coefficients close enough either way.
    return (0.114 * frame[..., 0] + 0.587 * frame[..., 1]
            + 0.299 * frame[..., 2]).astype(np.uint8)


def find_panel(gray: np.ndarray):
    """Locate the bright parchment Uncharted Waters box anywhere on screen.

    Reliable across the 5 real 4K frames: threshold the parchment, close the
    text holes, and pick the largest panel-shaped bright blob. Runs on a 4x
    downscale so the morphology is fast enough for the poll loop. Returns
    (x0, y0, x1, y1) in full-res pixels, or None.
    """
    step = 4
    small = gray[::step, ::step]
    mask = small > 150
    closed = ndimage.binary_closing(mask, structure=np.ones((3, 3)), iterations=2)
    lbl, n = ndimage.label(closed)
    if n == 0:
        return None
    sizes = ndimage.sum(closed, lbl, range(1, n + 1))
    slices = ndimage.find_objects(lbl)
    best = None
    for i, sl in enumerate(slices):
        if sl is None:
            continue
        ys, xs = sl
        bw = (xs.stop - xs.start) * step
        bh = (ys.stop - ys.start) * step
        fill = sizes[i] / max(1, (xs.stop - xs.start) * (ys.stop - ys.start))
        # Panel is ~620x390 at 4K; allow generous range, require a solid box.
        if 350 < bw < 900 and 250 < bh < 1000 and fill > 0.6:
            if best is None or sizes[i] > best[0]:
                best = (sizes[i], xs.start * step, ys.start * step,
                        xs.stop * step, ys.stop * step)
    if best is None:
        return None
    return best[1], best[2], best[3], best[4]


def _best_anchor(lines: list[Line], phrase: str, after_y: float = -1):
    key = _cnorm(phrase)
    best, best_s = None, 0.0
    for ln in lines:
        if ln.yc <= after_y:
            continue
        s = fuzz.ratio(_cnorm(ln.text), key)
        if s > best_s:
            best, best_s = ln, s
    return best, best_s


def _locate(lines: list[Line]):
    top, ts = _best_anchor(lines, ANCHOR_TOP)
    if top is None or ts < ANCHOR_MIN:
        return None
    bottom, bs = None, 0.0
    for phrase in ANCHOR_BOTTOM:
        b, s = _best_anchor(lines, phrase, after_y=top.yc)
        if s > bs:
            bottom, bs = b, s
    return top, (bottom if bs >= ANCHOR_MIN else None)


def _region(gray: np.ndarray, top: Line, bottom):
    h, w = gray.shape
    line_h = max(1, top.y1 - top.y0)
    y0 = top.y1                                   # just below the title
    y1 = bottom.y1 if bottom else top.y1 + 6 * line_h
    col_c = (top.x0 + top.x1) / 2
    col_half = max(top.x1 - top.x0, 260) * 0.95
    return (max(0, int(col_c - col_half)), max(0, int(y0)),
            min(w, int(col_c + col_half)), min(h, int(y1)))


class RumourRecognizer:
    def __init__(self, data: str | Path = _DATA):
        self.matcher = RumourMatcher(load_rumours(data))

    def recognize(self, frame: np.ndarray) -> list[RumourHit]:
        gray = _to_gray(frame)
        # Detection: anchor-locate the tooltip, OCR its region at several
        # scales, union the fuzzy matches (8/10 recall on real frames).
        loc = _locate(ocr_lines(gray, scale=DETECT_SCALE))
        if loc is None:
            return []
        top, bottom = loc
        rx0, ry0, rx1, ry1 = _region(gray, top, bottom)
        if rx1 <= rx0 or ry1 <= ry0:
            return []
        crop = gray[ry0:ry1, rx0:rx1]
        # Positioning: a tight bright-panel box gives the real right edge to
        # hang badges off; fall back to the anchor region if not found.
        pbox = find_panel(gray) or (rx0, ry0, rx1, ry1)
        hits: list[RumourHit] = []
        seen: set[str] = set()
        for scale in CROP_SCALES:
            for ln in sorted(ocr_lines(crop, scale=scale), key=lambda l: l.yc):
                m = self.matcher.match(ln.text)
                if m and m.name not in seen:
                    seen.add(m.name)
                    box = (rx0 + ln.x0, ry0 + ln.y0, rx0 + ln.x1, ry0 + ln.y1)
                    hits.append(RumourHit(m, box, ln.text, pbox))
        return hits
