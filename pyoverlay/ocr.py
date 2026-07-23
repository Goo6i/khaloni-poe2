"""Tesseract OCR via subprocess (no pytesseract dependency).

Phase 1 uses the system `tesseract` binary. A WinOCR backend can replace
this on Windows later behind the same ocr_lines() signature.
"""
from __future__ import annotations

import subprocess
import tempfile
from dataclasses import dataclass

import numpy as np
from PIL import Image


@dataclass
class Line:
    text: str
    x0: int
    y0: int
    x1: int
    y1: int

    @property
    def yc(self) -> float:
        return (self.y0 + self.y1) / 2


def _run_tsv(img: Image.Image, psm: int) -> str:
    with tempfile.NamedTemporaryFile(suffix=".png", delete=False) as f:
        img.save(f.name)
        path = f.name
    try:
        out = subprocess.run(
            ["tesseract", path, "-", "--psm", str(psm), "tsv"],
            capture_output=True, text=True, timeout=15)
        return out.stdout
    finally:
        import os
        os.unlink(path)


def ocr_lines(gray: np.ndarray, scale: float = 1.0, psm: int = 6) -> list[Line]:
    """OCR a grayscale image into text lines with bounding boxes.

    `scale` upsamples before OCR (tesseract reads larger text better);
    boxes are mapped back to the input coordinate space.
    """
    img = Image.fromarray(gray)
    if scale != 1.0:
        img = img.resize((max(1, int(img.width * scale)),
                          max(1, int(img.height * scale))), Image.LANCZOS)
    tsv = _run_tsv(img, psm)
    # Group TSV word rows (level 5) by (block, par, line).
    groups: dict[tuple, list] = {}
    for row in tsv.splitlines()[1:]:
        c = row.split("\t")
        if len(c) < 12 or c[0] != "5":
            continue
        try:
            conf = float(c[10])
        except ValueError:
            continue
        word = c[11].strip()
        if conf < 30 or not word:
            continue
        key = (c[2], c[3], c[4])
        x, y, w, h = int(c[6]), int(c[7]), int(c[8]), int(c[9])
        groups.setdefault(key, []).append((x, y, w, h, word))
    lines: list[Line] = []
    for words in groups.values():
        words.sort(key=lambda t: t[0])
        text = " ".join(w[4] for w in words)
        x0 = min(w[0] for w in words)
        y0 = min(w[1] for w in words)
        x1 = max(w[0] + w[2] for w in words)
        y1 = max(w[1] + w[3] for w in words)
        lines.append(Line(text, int(x0 / scale), int(y0 / scale),
                          int(x1 / scale), int(y1 / scale)))
    return lines
