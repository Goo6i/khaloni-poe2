"""Click-through overlay above the fullscreen game (Linux, GTK4 layer-shell).

A layer-shell surface covers the game output, sits in the overlay layer
(above the fullscreen game, exactly like the Rust tool's wlr-layer-shell),
takes no keyboard focus, and has an EMPTY input region so every click
falls through to the game. Cairo draws rating pills at the detected
rumour-line positions.

Coordinates: recognizers return boxes in game-monitor PHYSICAL pixels
(0..gw, 0..gh). The layer surface is sized in LOGICAL pixels, so boxes are
divided by `scale` (1.5 on the DP-2 4K panel) before drawing.

Windows/mac get a different overlay backend (always-on-top transparent
click-through window) in Phase 3; the draw() logic is shared.
"""
from __future__ import annotations

# gtk4-layer-shell MUST be loaded before gi pulls in libwayland-client, or
# init_for_window silently no-ops ("GtkWindow is not a layer surface").
# This is the canonical workaround from wmww/gtk4-layer-shell's own example.
from ctypes import CDLL

CDLL("libgtk4-layer-shell.so")

import gi  # noqa: E402

gi.require_version("Gtk", "4.0")
gi.require_version("Gtk4LayerShell", "1.0")
from gi.repository import Gtk, Gtk4LayerShell as LayerShell, GLib, Gdk  # noqa: E402


def _install_transparent_css():
    """Make the overlay window background transparent so only the cairo
    drawing shows (pattern from goodroot/hyprwhspr's layer-shell OSD)."""
    css = Gtk.CssProvider()
    css.load_from_string(".poe2lens-overlay { background-color: transparent; }")
    Gtk.StyleContext.add_provider_for_display(
        Gdk.Display.get_default(), css,
        Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION)


class Overlay:
    def __init__(self, scale: float = 1.5, monitor_index: int | None = None):
        self.scale = scale
        self.monitor_index = monitor_index
        self._hits = []           # list of (text, rating, box) tuples
        self.app = Gtk.Application(application_id="org.poe2lens.pyoverlay")
        self.app.connect("activate", self._on_activate)
        self._area = None

    # --- public API (thread-safe) --------------------------------------
    def set_rumours(self, hits) -> None:
        """Replace the drawn rumours. `hits` = list of RumourHit (kept whole
        so the shared render.draw_annotations can use box + panel)."""
        GLib.idle_add(self._apply, list(hits))

    def run(self) -> None:
        self.app.run(None)

    # --- internals -----------------------------------------------------
    def _apply(self, payload):
        self._hits = payload
        if self._area is not None:
            self._area.queue_draw()
        return False

    def _on_activate(self, app):
        _install_transparent_css()
        win = Gtk.ApplicationWindow(application=app)
        win.add_css_class("poe2lens-overlay")
        LayerShell.init_for_window(win)
        LayerShell.set_layer(win, LayerShell.Layer.OVERLAY)
        for edge in (LayerShell.Edge.TOP, LayerShell.Edge.BOTTOM,
                     LayerShell.Edge.LEFT, LayerShell.Edge.RIGHT):
            LayerShell.set_anchor(win, edge, True)
        LayerShell.set_keyboard_mode(win, LayerShell.KeyboardMode.NONE)
        LayerShell.set_exclusive_zone(win, -1)
        # Pin to a specific monitor (the game output) if requested.
        if self.monitor_index is not None:
            mons = win.get_display().get_monitors()
            if self.monitor_index < mons.get_n_items():
                LayerShell.set_monitor(win, mons.get_item(self.monitor_index))

        area = Gtk.DrawingArea()
        area.set_hexpand(True)
        area.set_vexpand(True)
        area.set_draw_func(self._draw)
        win.set_child(area)
        self._area = area

        win.present()
        # Empty input region = full click-through. Set after realize so the
        # surface exists.
        surface = win.get_surface()
        if surface is not None:
            import cairo
            region = cairo.Region()   # empty
            surface.set_input_region(region)

    def _draw(self, area, cr, width, height, *user_data):
        # Shared drawing: identical to the offline preview, so the live look
        # matches exactly. Boxes are in game-physical px; scale converts to
        # the logical layer surface.
        from .render import draw_annotations
        try:
            with open("/tmp/ov-draw.log", "a") as f:
                f.write(f"draw {width}x{height} hits={len(self._hits)}\n")
            draw_annotations(cr, self._hits, scale=self.scale)
        except Exception as e:
            import traceback
            with open("/tmp/ov-draw.log", "a") as f:
                f.write(f"DRAW ERROR: {e}\n{traceback.format_exc()}\n")
        return False
