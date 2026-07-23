"""Verify the rumour recognizer against the 5 real 4K fixtures.

Run: python3 -m pyoverlay.test_rumours   (from the repo root)
"""
import glob
import os
import sys

import numpy as np
from PIL import Image

sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
from pyoverlay.recognize.rumours import RumourRecognizer  # noqa: E402

# Ground truth per fixture (from the user's screenshots).
EXPECTED = {
    "rumour-1": {"Endless Cliffs"},
    "rumour-2": {"Bleak and Awful", "Warm but risky"},
    "rumour-3": {"Wild,.Roaming Free", "Cold as ice"},
    "rumour-4": {"Cold as ice", "Wild,.Roaming Free"},
    "rumour-5": {"Cold as ice", "Wild,.Roaming Free", "Warm but risky"},
}

FIX = os.path.join(os.path.dirname(__file__), "..", "app", "tests", "fixtures", "rumours")


def main():
    rec = RumourRecognizer()
    total_expected = total_found = total_correct = 0
    for path in sorted(glob.glob(os.path.join(FIX, "rumour-*.png"))):
        name = os.path.splitext(os.path.basename(path))[0]
        frame = np.asarray(Image.open(path).convert("RGB"))[..., ::-1]  # RGB->BGR
        hits = rec.recognize(frame)
        found = {h.rumour.name for h in hits}
        exp = EXPECTED.get(name, set())
        correct = found & exp
        total_expected += len(exp)
        total_found += len(found)
        total_correct += len(correct)
        got = ", ".join(f"{h.rumour.name} [{h.rumour.rating}]" for h in hits) or "-"
        print(f"{name}: {got}")
        missed = exp - found
        if missed:
            print(f"    missed: {missed}")
        false = found - exp
        if false:
            print(f"    FALSE POSITIVE: {false}")
    print(f"\nrecall {total_correct}/{total_expected}  "
          f"false-positives {total_found - total_correct}")


if __name__ == "__main__":
    main()
