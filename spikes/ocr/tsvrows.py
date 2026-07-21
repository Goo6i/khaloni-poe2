#!/usr/bin/env python3
"""Row extraction from tesseract TSV word boxes: filter by confidence,
group by line, fuzzy-match against expected vocabulary."""
import difflib
import re
import subprocess
import sys


def norm(line: str) -> str:
    line = line.strip().lower()
    line = re.sub(r"[^a-z0-9 ]", "", line)
    return re.sub(r"\s+", " ", line)


def ocr_rows(image: str, min_conf: float = 40.0) -> list[str]:
    tsv = subprocess.run(
        ["tesseract", image, "-", "--psm", "6", "-l", "eng", "tsv"],
        capture_output=True, text=True,
    ).stdout
    lines: dict[tuple, list[str]] = {}
    for row in tsv.splitlines()[1:]:
        f = row.split("\t")
        if len(f) < 12 or f[0] != "5":  # level 5 = word
            continue
        conf, text = float(f[10]), f[11].strip()
        if conf < min_conf or not text:
            continue
        lines.setdefault((f[2], f[3], f[4]), []).append(text)  # block, par, line
    return [n for words in lines.values() if (n := norm(" ".join(words)))]


def main(sample: str) -> None:
    reads = ocr_rows(f"{sample}.pre2.png")
    reads_all = ocr_rows(f"{sample}.pre2.png", min_conf=-10)
    expected = [norm(l) for l in open(f"{sample}.expected.txt") if norm(l)]
    hits = 0
    for e in expected:
        best = max((difflib.SequenceMatcher(None, e, r).ratio() for r in reads), default=0)
        if any(e in r for r in reads + reads_all) or best >= 0.75:
            hits += 1
        else:
            print(f"  MISS: {e!r} (best ratio {best:.2f})")
    pct = 100.0 * hits / max(len(expected), 1)
    print(f"{sample}: {hits}/{len(expected)} rows matched ({pct:.0f}%)")


if __name__ == "__main__":
    main(sys.argv[1])
