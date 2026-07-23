"""Cross-platform frame capture.

Backends:
  - Windows: dxcam / mss  (trivial; added in Phase 3).
  - Linux non-gamescope: mss (X11).
  - Linux gamescope/Wayland (this box): `spectacle` full-desktop grab
    cropped to the game output. This sidesteps the portal that Rust
    needed; a persistent portal/pipewire grabber can replace it later if
    spectacle's per-frame process spawn is too slow for the fast reward
    path (it is fine for the 1-2 Hz rumour path).

Returns frames as HxWx3 BGR uint8 (OpenCV convention; recognizers accept
BGR or gray).
"""
from __future__ import annotations

import subprocess
import tempfile
import os

import numpy as np
from PIL import Image


class SpectacleCapture:
    """Linux/gamescope backend: spectacle full grab, crop to game output.

    game_rect is (x0, y0, x1, y1) in the full-desktop physical pixel space.
    Pinned for this box (DP-2 game monitor). Generalize via kscreen-doctor
    geometry later.
    """

    def __init__(self, game_rect: tuple[int, int, int, int] = (6400, 0, 10240, 2160)):
        self.rect = game_rect

    def grab(self) -> np.ndarray:
        with tempfile.NamedTemporaryFile(suffix=".png", delete=False) as f:
            path = f.name
        try:
            subprocess.run(["spectacle", "-b", "-n", "-f", "-o", path],
                           capture_output=True, timeout=10)
            im = Image.open(path).convert("RGB")
            x0, y0, x1, y1 = self.rect
            im = im.crop((x0, y0, x1, y1))
            rgb = np.asarray(im)
            return rgb[..., ::-1].copy()   # RGB -> BGR
        finally:
            if os.path.exists(path):
                os.unlink(path)


def default_capture():
    """Pick a backend for the current platform (Phase 1: Linux spectacle)."""
    import sys
    if sys.platform.startswith("linux"):
        return SpectacleCapture()
    raise NotImplementedError("Windows/mac backends land in Phase 3")


if __name__ == "__main__":
    cap = default_capture()
    import time
    t = time.time()
    frame = cap.grab()
    print(f"grabbed {frame.shape} in {time.time()-t:.2f}s")
    Image.fromarray(frame[..., ::-1]).save("/tmp/pyoverlay-grab.png")
    print("saved /tmp/pyoverlay-grab.png")
