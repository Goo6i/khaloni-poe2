"""Fuzzy matching of OCR text against the closed rumour vocabulary.

Mirrors danielmtv2/poe2-expedition-overlay's approach: class-normalize the
text (collapse visually-confusable glyph classes so OCR noise lands near
the truth), then rapidfuzz against the known rumour names with a score
threshold and a runner-up margin so ambiguous near-ties are dropped rather
than guessed. A chrome guard rejects tooltip header/footer text that can
resemble a rumour.
"""
from __future__ import annotations

import csv
import re
from dataclasses import dataclass
from pathlib import Path

from rapidfuzz import fuzz, process

MATCH_THRESHOLD = 70.0   # rapidfuzz ratio 0-100 (danielmtv2: 70)
MATCH_MARGIN = 5.0       # best must beat runner-up by this (danielmtv2: 5)

# Text that lives inside the cropped rumour region but is NOT a rumour;
# guarded so it never produces a phantom match.
CHROME = ["ISLAND RUMOURS", "USE A LOGBOOK TO CHART THE AREA",
          "UNCHARTED WATERS", "EXPEDITION LOGBOOK", "REQUIRES", "CONSUMES"]

# Glyph classes: characters OCR routinely confuses map to one representative
# so "wild voaming fvee" and "wild roaming free" collapse together.
_CLASS = str.maketrans({
    "v": "u", "w": "u", "m": "n", "r": "n",
    "i": "l", "j": "l", "t": "l", "1": "l",
    "0": "o", "e": "o", "c": "o",
    "5": "s", "8": "b",
})


def _norm(s: str) -> str:
    return re.sub(r"[^a-z0-9]", "", s.lower())


def _cnorm(s: str) -> str:
    """Class-normalized key: lowercase, strip non-alnum, collapse classes."""
    return _norm(s).translate(_CLASS)


@dataclass(frozen=True)
class Rumour:
    name: str
    map_type: str
    mods: str
    rating: str


def load_rumours(path: str | Path) -> list[Rumour]:
    out: list[Rumour] = []
    with open(path, newline="") as f:
        for row in csv.DictReader(f):
            name = (row.get("Rumor") or "").strip()
            if not name:
                continue
            out.append(Rumour(name,
                              (row.get("Map Type") or "").strip(),
                              (row.get("Mods") or "").strip(),
                              (row.get("Rating") or "").strip()))
    return out


class RumourMatcher:
    def __init__(self, rumours: list[Rumour]):
        self.rumours = rumours
        self._by_key = {_cnorm(r.name): r for r in rumours}
        self._choices = list(self._by_key.keys())
        self._chrome = [_cnorm(c) for c in CHROME]

    def match(self, text: str) -> Rumour | None:
        key = _cnorm(text)
        if len(key) < 4:
            return None
        ranked = process.extract(key, self._choices, scorer=fuzz.ratio, limit=2)
        if not ranked:
            return None
        choice, score, _ = ranked[0]
        if score < MATCH_THRESHOLD:
            return None
        if len(ranked) > 1 and score - ranked[1][1] < MATCH_MARGIN:
            return None
        # Reject if tooltip chrome matches at least as well.
        chrome = process.extractOne(key, self._chrome, scorer=fuzz.ratio)
        if chrome is not None and chrome[1] >= score:
            return None
        return self._by_key[choice]
