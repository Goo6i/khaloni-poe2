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


def run_overlay(cap, rec, interval: float, scale: float, monitor: int) -> None:
    """On-screen mode: a background thread polls capture+recognize and pushes
    hits to the GTK layer-shell overlay, which redraws the rating pills."""
    import threading
    from .overlay import Overlay

    ov = Overlay(scale=scale, monitor_index=monitor)

    def log(msg):
        with open("/tmp/ov-worker.log", "a") as f:
            f.write(msg + "\n")

    def worker():
        log("worker thread started")
        n = 0
        while True:
            n += 1
            log(f"poll {n} start (grabbing)")
            try:
                frame = cap.grab()
                hits = rec.recognize(frame)
                ov.set_rumours(hits)
                log(f"poll {n}: frame={frame.shape} hits={len(hits)} "
                    + ", ".join(f"{h.rumour.name}[{h.rumour.rating}]" for h in hits))
            except Exception as e:
                import traceback
                log(f"poll {n} ERROR: {e}\n{traceback.format_exc()}")
            time.sleep(interval)

    threading.Thread(target=worker, daemon=True).start()
    ov.run()


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--loop", type=float, default=0,
                    help="poll interval in seconds (0 = one-shot console)")
    ap.add_argument("--overlay", action="store_true",
                    help="draw on-screen (GTK layer-shell) instead of console")
    ap.add_argument("--interval", type=float, default=2.0,
                    help="overlay poll interval seconds")
    ap.add_argument("--scale", type=float, default=1.5,
                    help="game physical / logical pixel ratio (DP-2 4K = 1.5)")
    ap.add_argument("--monitor", type=int, default=0,
                    help="GTK monitor index of the game output (DP-2 = 0)")
    args = ap.parse_args()

    cap = default_capture()
    rec = RumourRecognizer()
    if args.overlay:
        run_overlay(cap, rec, args.interval, args.scale, args.monitor)
        return
    if args.loop <= 0:
        run_once(cap, rec)
        return
    print(f"polling every {args.loop}s; Ctrl+C to stop")
    while True:
        run_once(cap, rec)
        time.sleep(args.loop)


if __name__ == "__main__":
    main()
