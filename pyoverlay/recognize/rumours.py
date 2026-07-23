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


def _to_gray(frame: np.ndarray) -> np.ndarray:
    if frame.ndim == 2:
        return frame
    # BGR or RGB -> luma; coefficients close enough either way.
    return (0.114 * frame[..., 0] + 0.587 * frame[..., 1]
            + 0.299 * frame[..., 2]).astype(np.uint8)


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
        loc = _locate(ocr_lines(gray, scale=DETECT_SCALE))
        if loc is None:
            return []
        top, bottom = loc
        x0, y0, x1, y1 = _region(gray, top, bottom)
        if x1 <= x0 or y1 <= y0:
            return []
        crop = gray[y0:y1, x0:x1]
        hits: list[RumourHit] = []
        seen: set[str] = set()
        for scale in CROP_SCALES:
            for ln in sorted(ocr_lines(crop, scale=scale), key=lambda l: l.yc):
                m = self.matcher.match(ln.text)
                if m and m.name not in seen:
                    seen.add(m.name)
                    box = (x0 + ln.x0, y0 + ln.y0, x0 + ln.x1, y0 + ln.y1)
                    hits.append(RumourHit(m, box, ln.text))
        return hits
