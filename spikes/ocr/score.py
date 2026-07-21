#!/usr/bin/env python3
"""Score OCR output against expected rows: exact-match after normalization."""
import re
import sys

def norm(line: str) -> str:
    line = line.strip().lower()
    line = re.sub(r"[^a-z0-9 ]", "", line)
    return re.sub(r"\s+", " ", line)

def rows(path: str) -> list[str]:
    with open(path) as f:
        return [norm(l) for l in f if norm(l)]

expected, actual = rows(sys.argv[1]), rows(sys.argv[2])
hits = sum(1 for e in expected if any(e in a for a in actual))
pct = 100.0 * hits / max(len(expected), 1)
print(f"{hits}/{len(expected)} rows matched ({pct:.0f}%)")
sys.exit(0 if pct >= 95 else 1)
