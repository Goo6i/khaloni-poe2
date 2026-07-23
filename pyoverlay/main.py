"""End-to-end runner (Phase 1, console output).

Captures the live game frame, recognizes rumours (and reward prices once
rewards.py lands), prints them. Proves the capture -> recognize pipeline
on real frames before the PyQt overlay (Phase 2).

  python3 -m pyoverlay.main            # one-shot
  python3 -m pyoverlay.main --loop 2   # every 2s until Ctrl+C
"""
from __future__ import annotations

import argparse
import time

from .capture import default_capture
from .recognize.rumours import RumourRecognizer


def run_once(cap, rec) -> None:
    t = time.time()
    frame = cap.grab()
    hits = rec.recognize(frame)
    dt = time.time() - t
    if hits:
        print(f"[{dt:.1f}s] {len(hits)} rumour(s):")
        for h in hits:
            r = h.rumour
            print(f"   {r.rating:>3}  {r.name:<22} {r.map_type} / {r.mods}")
    else:
        print(f"[{dt:.1f}s] no rumour panel in view")


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--loop", type=float, default=0,
                    help="poll interval in seconds (0 = one-shot)")
    args = ap.parse_args()

    cap = default_capture()
    rec = RumourRecognizer()
    if args.loop <= 0:
        run_once(cap, rec)
        return
    print(f"polling every {args.loop}s; Ctrl+C to stop")
    while True:
        run_once(cap, rec)
        time.sleep(args.loop)


if __name__ == "__main__":
    main()
