use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, Region},
    delegate_compositor, delegate_keyboard, delegate_layer, delegate_output, delegate_pointer,
    delegate_registry, delegate_seat, delegate_shm,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    seat::{
        keyboard::{KeyEvent, KeyboardHandler, Keysym, Modifiers},
        pointer::{PointerEvent, PointerEventKind, PointerHandler},
        Capability, SeatHandler, SeatState,
    },
    shell::{
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
        WaylandSurface,
    },
    shm::{slot::SlotPool, Shm, ShmHandler},
};
use smithay_client_toolkit::reexports::protocols::wp::{
    fractional_scale::v1::client::{
        wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1,
        wp_fractional_scale_v1::{self, WpFractionalScaleV1},
    },
    viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter},
};
use tiny_skia::Pixmap;
use wayland_client::{
    delegate_noop,
    globals::registry_queue_init,
    protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
    Connection, Dispatch, EventQueue, QueueHandle,
};

pub use crate::platform::{Key, OverlayError};

struct App {
    registry_state: RegistryState,
    output_state: OutputState,
    seat_state: SeatState,
    shm: Shm,
    pool: SlotPool,
    layer: Option<LayerSurface>,
    surface_size: (u32, u32),
    pointer: Option<wl_pointer::WlPointer>,
    keyboard: Option<wl_keyboard::WlKeyboard>,
    /// Left-button presses on the overlay surface (surface-local logical
    /// px), drained by the main loop. Only non-empty while an interactive
    /// input region is set: with the default empty region the compositor
    /// never routes pointer input here at all.
    clicks: Vec<(i32, i32)>,
    /// Latest pointer position (surface-local logical px), updated on every
    /// motion/press event. Used to drive panel dragging.
    pointer_pos: (i32, i32),
    /// Whether the left button is currently held. Turns a press+motion+release
    /// into a drag without threading individual events through the main loop.
    button_down: bool,
    /// Editing keystrokes, drained by the main loop while a box is focused.
    keys: Vec<Key>,
    /// Key currently held for synthetic repeat: its raw code (to match the
    /// release event), the mapped key, and when it next fires. Wayland
    /// delivers exactly one press per physical press, so holding Backspace
    /// only streams deletes if the client synthesizes the repeats itself.
    held: Option<(u32, Key, std::time::Instant)>,
    /// Compositor-announced repeat timing (initial delay, then interval);
    /// None when the compositor disabled repeat.
    repeat: Option<(std::time::Duration, std::time::Duration)>,
    /// The compositor closed the layer surface (its output was switched
    /// off or unplugged). Nothing can be drawn on it again; see
    /// `Overlay::is_closed`.
    closed: bool,
    /// Scales a buffer of any size onto the surface's logical size; what
    /// makes a fractional device-pixel buffer possible. None when the
    /// compositor lacks wp_viewporter.
    viewport: Option<WpViewport>,
    /// Kept alive for its preferred_scale events.
    fractional: Option<WpFractionalScaleV1>,
    /// The compositor's preferred scale in 120ths (180 = 150%), from
    /// wp_fractional_scale_v1. None until announced or when unsupported.
    scale120: Option<u32>,
    /// Integer scale from wl_surface / the outputs entered: the fallback
    /// when fractional scaling is unavailable.
    int_scale: i32,
    /// The wl_surface buffer scale last committed.
    buffer_scale: i32,
}

/// Index of the output rect `(x, y, w, h)` holding `point`, else the one
/// nearest to it: a game window dragged half off-screen, or a point
/// remembered from an output that no longer exists, still lands on a real
/// output with a known origin.
pub fn pick_output(point: (i32, i32), outputs: &[(i32, i32, i32, i32)]) -> Option<usize> {
    let distance = |&(x, y, w, h): &(i32, i32, i32, i32)| -> i64 {
        let dx = (x - point.0).max(point.0 - (x + w - 1)).max(0) as i64;
        let dy = (y - point.1).max(point.1 - (y + h - 1)).max(0) as i64;
        dx * dx + dy * dy
    };
    outputs
        .iter()
        .enumerate()
        .filter(|(_, o)| o.2 > 0 && o.3 > 0)
        .min_by_key(|(_, o)| distance(o))
        .map(|(i, _)| i)
}

/// Buffer pixels for `logical` surface units at `scale120`/120, rounded
/// half away from zero as wp_fractional_scale_v1 specifies.
pub fn scaled(logical: u32, scale120: u32) -> u32 {
    ((logical as u64 * scale120 as u64 + 60) / 120) as u32
}

pub struct Overlay {
    _conn: Connection,
    event_queue: EventQueue<App>,
    app: App,
    output_pos: (i32, i32),
    compositor: CompositorState,
    /// Global opacity applied to every presented pixel; see `set_opacity`.
    opacity: f64,
    /// Mirrors the keyboard-interactivity state for the game-window feed
    /// (see `bind_keyboard_flag`).
    keyboard_flag: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}

impl Overlay {
    /// The overlay on the output holding `target_center` (global logical
    /// px). Startup flavour: with no output known it still creates the
    /// surface and lets the compositor place it.
    pub fn new(target_center: (i32, i32)) -> anyhow::Result<Overlay> {
        Ok(Self::build(target_center, false)?)
    }

    /// Like `new`, for rebuilding after `is_closed()`: fails with
    /// `OverlayError::NoOutput` while no output exists, so the caller can
    /// retry until a monitor is back. The new overlay starts from defaults:
    /// `bind_keyboard_flag` and `set_opacity` have to be applied again.
    pub fn open(target_center: (i32, i32)) -> Result<Overlay, OverlayError> {
        Self::build(target_center, true)
    }

    fn build(target_center: (i32, i32), require_output: bool) -> Result<Overlay, OverlayError> {
        let startup = |e: &dyn std::fmt::Display| OverlayError::Startup(e.to_string());
        let conn = Connection::connect_to_env().map_err(|e| startup(&e))?;
        let (globals, event_queue) = registry_queue_init(&conn).map_err(|e| startup(&e))?;
        let qh: QueueHandle<App> = event_queue.handle();

        let compositor = CompositorState::bind(&globals, &qh).map_err(|e| startup(&e))?;
        let layer_shell = LayerShell::bind(&globals, &qh).map_err(|e| startup(&e))?;
        let shm = Shm::bind(&globals, &qh).map_err(|e| startup(&e))?;
        // Both optional. Fractional scaling is only usable together with a
        // viewport (the buffer is then not an integer multiple of the
        // surface), so the manager is not even bound without one.
        let viewporter: Option<WpViewporter> = globals.bind(&qh, 1..=1, ()).ok();
        let fractional_manager: Option<WpFractionalScaleManagerV1> = match viewporter {
            Some(_) => globals.bind(&qh, 1..=1, ()).ok(),
            None => None,
        };

        let pool = SlotPool::new(1024 * 1024, &shm).map_err(|e| startup(&e))?;
        let mut app = App {
            registry_state: RegistryState::new(&globals),
            output_state: OutputState::new(&globals, &qh),
            seat_state: SeatState::new(&globals, &qh),
            shm,
            pool,
            layer: None,
            surface_size: (0, 0),
            pointer: None,
            keyboard: None,
            clicks: Vec::new(),
            pointer_pos: (0, 0),
            button_down: false,
            keys: Vec::new(),
            held: None,
            // Sane defaults until the compositor announces its own timing.
            repeat: Some((
                std::time::Duration::from_millis(400),
                std::time::Duration::from_millis(35),
            )),
            closed: false,
            viewport: None,
            fractional: None,
            scale120: None,
            int_scale: 1,
            buffer_scale: 1,
        };

        let mut event_queue = event_queue;

        // Two roundtrips so OutputState is populated, then pick the output containing
        // the tracked window's center.
        event_queue.roundtrip(&mut app).map_err(|e| startup(&e))?;
        event_queue.roundtrip(&mut app).map_err(|e| startup(&e))?;

        let outputs: Vec<wl_output::WlOutput> = app.output_state.outputs().collect();
        if outputs.is_empty() && require_output {
            return Err(OverlayError::NoOutput);
        }
        let rects: Vec<(i32, i32, i32, i32)> = outputs
            .iter()
            .map(|o| {
                let info = app.output_state.info(o);
                let pos = info.as_ref().and_then(|i| i.logical_position).unwrap_or_default();
                let size = info.as_ref().and_then(|i| i.logical_size).unwrap_or_default();
                (pos.0, pos.1, size.0, size.1)
            })
            .collect();
        let picked = pick_output(target_center, &rects);
        let target = picked.map(|i| outputs[i].clone());
        let output_pos = picked.map(|i| (rects[i].0, rects[i].1)).unwrap_or((0, 0));

        let surface = compositor.create_surface(&qh);
        let layer = layer_shell.create_layer_surface(
            &qh,
            surface,
            Layer::Overlay,
            Some("khaloni-poe2"),
            target.as_ref(),
        );
        // Cover the whole output.
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);

        if let Some(vp) = &viewporter {
            app.viewport = Some(vp.get_viewport(layer.wl_surface(), &qh, ()));
        }
        if let Some(fm) = &fractional_manager {
            app.fractional = Some(fm.get_fractional_scale(layer.wl_surface(), &qh, ()));
        }

        // Empty input region = every click falls through to whatever is beneath.
        let region = Region::new(&compositor).map_err(|e| startup(&e))?;
        layer.wl_surface().set_input_region(Some(region.wl_region()));
        layer.commit();
        app.layer = Some(layer);

        Ok(Overlay {
            _conn: conn,
            event_queue,
            app,
            output_pos,
            compositor,
            opacity: 1.0,
            keyboard_flag: None,
        })
    }

    /// True once the compositor closed the surface: its output was switched
    /// off, unplugged, or replugged. Every other method is a no-op from
    /// then on and nothing is drawn; build a new overlay with `open` (it
    /// reports `NoOutput` until a monitor is back) and drop this one.
    pub fn is_closed(&self) -> bool {
        self.app.closed
    }

    /// Device pixels per logical pixel on the overlay's output (1.5 on a
    /// 150% display). 1.0 until the compositor announces it, which happens
    /// shortly after the first frame; it can change at any time, so read it
    /// per frame.
    pub fn scale(&self) -> f64 {
        match self.app.scale120 {
            Some(s) if self.app.viewport.is_some() => s as f64 / 120.0,
            _ => self.app.int_scale.max(1) as f64,
        }
    }

    /// `size()` in device pixels: the pixmap size at which `present` maps
    /// one pixmap pixel to one screen pixel.
    pub fn device_size(&self) -> (u32, u32) {
        let (w, h) = self.app.surface_size;
        match self.app.scale120 {
            Some(s) if self.app.viewport.is_some() => (scaled(w, s), scaled(h, s)),
            _ => {
                let s = self.app.int_scale.max(1) as u32;
                (w * s, h * s)
            }
        }
    }

    /// Shares the keyboard-interactivity state with the game-window feed.
    /// KWin activates a layer surface the moment it asks for keyboard focus
    /// (`OnDemand`) and does nothing when it gives it back, so the game
    /// stays unfocused after a panel closes; the KWin script reads this
    /// flag and re-activates the game once it drops to false.
    pub fn bind_keyboard_flag(&mut self, flag: std::sync::Arc<std::sync::atomic::AtomicBool>) {
        self.keyboard_flag = Some(flag);
    }

    /// Makes `rect` (surface-local logical px) accept pointer input, or
    /// restores full click-through with None. The wl_region contents are
    /// copied by set_input_region, so the Region can drop right after.
    pub fn set_interactive(&mut self, rect: Option<(i32, i32, u32, u32)>) -> anyhow::Result<()> {
        let Some(layer) = self.app.layer.as_ref() else {
            self.app.clicks.clear();
            return Ok(());
        };
        let region = Region::new(&self.compositor)?;
        if let Some((x, y, w, h)) = rect {
            region.add(x, y, w as i32, h as i32);
        } else {
            // Dropping the region also drops any stale clicks nobody
            // drained (a click raced the panel closing).
            self.app.clicks.clear();
        }
        layer.wl_surface().set_input_region(Some(region.wl_region()));
        layer.commit();
        Ok(())
    }

    /// Drains left-clicks received since the last call (surface-local).
    pub fn take_clicks(&mut self) -> Vec<(i32, i32)> {
        std::mem::take(&mut self.app.clicks)
    }

    /// Latest pointer position, surface-local logical px.
    pub fn pointer_pos(&self) -> (i32, i32) {
        self.app.pointer_pos
    }

    /// Whether the left mouse button is currently held (for drag tracking).
    pub fn button_down(&self) -> bool {
        self.app.button_down
    }

    /// Drains editing keystrokes received since the last call, including
    /// synthetic repeats for a held key (Wayland sends one press only).
    pub fn take_keys(&mut self) -> Vec<Key> {
        if let (Some((_, key, next)), Some((_, interval))) =
            (&mut self.app.held, self.app.repeat)
        {
            let now = std::time::Instant::now();
            while *next <= now {
                self.app.keys.push(*key);
                *next += interval;
            }
        }
        std::mem::take(&mut self.app.keys)
    }

    /// Requests (or releases) keyboard focus for the overlay surface, so a
    /// focused value box can receive typed digits. On-demand: the compositor
    /// grants focus on the pointer interaction that opened the box.
    pub fn set_keyboard(&mut self, on: bool) -> anyhow::Result<()> {
        // Published before the commit, so the feed never sees the surface
        // active with the flag still saying "no keyboard wanted".
        if let Some(flag) = &self.keyboard_flag {
            flag.store(on, std::sync::atomic::Ordering::Relaxed);
        }
        if let Some(layer) = self.app.layer.as_ref() {
            layer.set_keyboard_interactivity(if on {
                KeyboardInteractivity::OnDemand
            } else {
                KeyboardInteractivity::None
            });
            layer.commit();
        }
        if !on {
            self.app.keys.clear();
        }
        Ok(())
    }

    pub fn pump(&mut self) -> anyhow::Result<()> {
        self.event_queue
            .roundtrip(&mut self.app)
            .map_err(|e| OverlayError::Connection(e.to_string()))?;
        Ok(())
    }

    /// Shows `pixmap` across the whole surface. Sized `size()` it is drawn
    /// in logical pixels and the compositor scales it up on a scaled
    /// display (soft text); sized `device_size()` it reaches the screen
    /// pixel for pixel.
    pub fn present(&mut self, pixmap: &Pixmap) -> anyhow::Result<()> {
        let (lw, lh) = self.app.surface_size;
        if lw == 0 || lh == 0 || self.app.layer.is_none() {
            return Ok(());
        }
        let (w, h) = (pixmap.width(), pixmap.height());
        let buffer_scale = if self.app.viewport.is_some() {
            // The viewport maps whatever the buffer is onto the logical
            // size, so the buffer scale stays out of it.
            1
        } else if (w, h) == (lw, lh) {
            1
        } else {
            let s = self.app.int_scale.max(1);
            if (w, h) != (lw * s as u32, lh * s as u32) {
                // Rendered for a size the surface no longer has (a
                // configure arrived in between); the next frame fits.
                return Ok(());
            }
            s
        };
        let stride = (w * 4) as i32;
        let (buffer, canvas) = self
            .app
            .pool
            .create_buffer(w as i32, h as i32, stride, wl_shm::Format::Argb8888)?;
        let src = pixmap.data();
        // tiny-skia stores premultiplied RGBA; wl ARGB8888 little-endian wants
        // [B,G,R,A]. The global opacity scales every channel (premultiplied,
        // so alpha and color fade together) — a 256-entry table keeps the
        // per-pixel cost to a lookup.
        let lut: [u8; 256] = {
            let mut t = [0u8; 256];
            let o = self.opacity.clamp(0.0, 1.0);
            for (i, v) in t.iter_mut().enumerate() {
                *v = (i as f64 * o) as u8;
            }
            t
        };
        for (dst, s) in canvas.as_chunks_mut::<4>().0.iter_mut().zip(src.as_chunks::<4>().0) {
            dst[0] = lut[s[2] as usize];
            dst[1] = lut[s[1] as usize];
            dst[2] = lut[s[0] as usize];
            dst[3] = lut[s[3] as usize];
        }
        let Some(layer) = self.app.layer.as_ref() else {
            return Ok(());
        };
        if let Some(viewport) = &self.app.viewport {
            viewport.set_destination(lw as i32, lh as i32);
        }
        if buffer_scale != self.app.buffer_scale {
            layer.wl_surface().set_buffer_scale(buffer_scale);
            self.app.buffer_scale = buffer_scale;
        }
        layer.wl_surface().damage_buffer(0, 0, w as i32, h as i32);
        buffer.attach_to(layer.wl_surface())?;
        layer.commit();
        Ok(())
    }

    pub fn hide(&mut self) -> anyhow::Result<()> {
        let (w, h) = self.app.surface_size;
        if w == 0 || h == 0 {
            return Ok(());
        }
        let all_zero = Pixmap::new(w, h).ok_or_else(|| anyhow::anyhow!("pixmap alloc failed"))?;
        self.present(&all_zero)?;
        Ok(())
    }

    pub fn size(&self) -> (u32, u32) {
        self.app.surface_size
    }

    pub fn output_pos(&self) -> (i32, i32) {
        self.output_pos
    }

    /// Global overlay opacity (0.0..=1.0), applied at present time. The
    /// caller repaints after changing it; an idle overlay keeps its last
    /// buffer until then.
    pub fn set_opacity(&mut self, opacity: f64) {
        self.opacity = opacity;
    }
}

impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {
        // Sent when the surface's output goes away. The surface is dead for
        // good - a replugged monitor is a new output - so release it and
        // let the owner see `is_closed()`.
        self.closed = true;
        if let Some(f) = self.fractional.take() {
            f.destroy();
        }
        if let Some(v) = self.viewport.take() {
            v.destroy();
        }
        self.layer = None;
        self.surface_size = (0, 0);
        self.clicks.clear();
        self.keys.clear();
        self.held = None;
        self.button_down = false;
    }
    fn configure(
        &mut self,
        _: &Connection,
        _qh: &QueueHandle<Self>,
        _: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _serial: u32,
    ) {
        self.surface_size = configure.new_size;
    }
}

impl CompositorHandler for App {
    fn scale_factor_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, factor: i32) {
        self.int_scale = factor.max(1);
    }
    fn transform_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: wl_output::Transform) {}
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}
    fn surface_enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
    fn surface_leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl SeatHandler for App {
    fn seat_state(&mut self) -> &mut SeatState {
        &mut self.seat_state
    }
    fn new_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
    fn new_capability(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        seat: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer && self.pointer.is_none() {
            self.pointer = self.seat_state.get_pointer(qh, &seat).ok();
        }
        if capability == Capability::Keyboard && self.keyboard.is_none() {
            self.keyboard = self.seat_state.get_keyboard(qh, &seat, None).ok();
        }
    }
    fn remove_capability(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: wl_seat::WlSeat,
        capability: Capability,
    ) {
        if capability == Capability::Pointer {
            if let Some(p) = self.pointer.take() {
                p.release();
            }
        }
        if capability == Capability::Keyboard {
            if let Some(k) = self.keyboard.take() {
                k.release();
            }
        }
    }
    fn remove_seat(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_seat::WlSeat) {}
}

impl PointerHandler for App {
    fn pointer_frame(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_pointer::WlPointer,
        events: &[PointerEvent],
    ) {
        const BTN_LEFT: u32 = 0x110;
        for ev in events {
            self.pointer_pos = (ev.position.0 as i32, ev.position.1 as i32);
            match ev.kind {
                PointerEventKind::Press { button: BTN_LEFT, .. } => {
                    self.clicks.push((ev.position.0 as i32, ev.position.1 as i32));
                    self.button_down = true;
                }
                PointerEventKind::Release { button: BTN_LEFT, .. } => {
                    self.button_down = false;
                }
                _ => {}
            }
        }
    }
}

impl KeyboardHandler for App {
    fn enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: &wl_surface::WlSurface,
        _: u32,
        _: &[u32],
        _: &[Keysym],
    ) {
    }
    fn leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: &wl_surface::WlSurface,
        _: u32,
    ) {
        self.held = None;
    }
    fn press_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        match event.keysym {
            Keysym::BackSpace => self.keys.push(Key::Backspace),
            Keysym::Return | Keysym::KP_Enter => self.keys.push(Key::Enter),
            Keysym::Escape => self.keys.push(Key::Escape),
            _ => {
                if let Some(c) = event.utf8.as_ref().and_then(|s| s.chars().next()) {
                    if c.is_ascii_digit() {
                        self.keys.push(Key::Digit(c));
                    } else if c == '.' {
                        self.keys.push(Key::Dot);
                    } else if c.is_ascii_graphic() || c == ' ' {
                        self.keys.push(Key::Char(c));
                    }
                }
            }
        }
        // Arm the repeat with whatever the press produced. Enter and Escape
        // stay single-shot: repeating a commit or a close is never what
        // holding the key means.
        if let Some(&k) = self.keys.last() {
            if !matches!(k, Key::Enter | Key::Escape) {
                if let Some((delay, _)) = self.repeat {
                    self.held = Some((event.raw_code, k, std::time::Instant::now() + delay));
                }
            }
        }
    }
    fn release_key(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        event: KeyEvent,
    ) {
        if self.held.as_ref().is_some_and(|(rc, _, _)| *rc == event.raw_code) {
            self.held = None;
        }
    }
    fn update_repeat_info(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        info: smithay_client_toolkit::seat::keyboard::RepeatInfo,
    ) {
        use smithay_client_toolkit::seat::keyboard::RepeatInfo;
        self.repeat = match info {
            RepeatInfo::Repeat { rate, delay } => Some((
                std::time::Duration::from_millis(u64::from(delay)),
                std::time::Duration::from_secs(1) / rate.get(),
            )),
            RepeatInfo::Disable => None,
        };
    }

    fn update_modifiers(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_keyboard::WlKeyboard,
        _: u32,
        _: Modifiers,
        _: u32,
    ) {
    }
}

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState, SeatState];
}

impl Dispatch<WpFractionalScaleV1, ()> for App {
    fn event(
        state: &mut Self,
        _: &WpFractionalScaleV1,
        event: wp_fractional_scale_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wp_fractional_scale_v1::Event::PreferredScale { scale } = event {
            state.scale120 = (scale > 0).then_some(scale);
        }
    }
}

// None of these three sends events.
delegate_noop!(App: WpViewporter);
delegate_noop!(App: WpViewport);
delegate_noop!(App: WpFractionalScaleManagerV1);

delegate_compositor!(App);
delegate_output!(App);
delegate_shm!(App);
delegate_seat!(App);
delegate_keyboard!(App);
delegate_pointer!(App);
delegate_layer!(App);
delegate_registry!(App);

#[cfg(test)]
mod tests {
    use super::{pick_output, scaled};

    const LEFT: (i32, i32, i32, i32) = (0, 0, 2560, 1440);
    const RIGHT: (i32, i32, i32, i32) = (2560, 0, 2560, 1440);

    #[test]
    fn the_output_holding_the_point_wins() {
        assert_eq!(pick_output((3840, 720), &[LEFT, RIGHT]), Some(1));
        assert_eq!(pick_output((100, 100), &[LEFT, RIGHT]), Some(0));
        // The shared edge belongs to the output that starts there.
        assert_eq!(pick_output((2560, 0), &[LEFT, RIGHT]), Some(1));
    }

    #[test]
    fn a_point_on_no_output_goes_to_the_nearest_one() {
        // The right monitor was unplugged; the game is remembered there.
        assert_eq!(pick_output((3840, 720), &[LEFT]), Some(0));
        assert_eq!(pick_output((6000, 3000), &[LEFT, RIGHT]), Some(1));
        assert_eq!(pick_output((-50, 700), &[LEFT, RIGHT]), Some(0));
    }

    #[test]
    fn outputs_without_geometry_are_not_candidates() {
        assert_eq!(pick_output((10, 10), &[]), None);
        assert_eq!(pick_output((10, 10), &[(0, 0, 0, 0)]), None);
        assert_eq!(pick_output((10, 10), &[(0, 0, 0, 0), RIGHT]), Some(1));
    }

    #[test]
    fn device_size_rounds_like_the_protocol() {
        // 150% on a 4K panel: 2560x1440 logical is the full 3840x2160.
        assert_eq!((scaled(2560, 180), scaled(1440, 180)), (3840, 2160));
        assert_eq!(scaled(1000, 120), 1000);
        // Halves round away from zero: 1001 * 1.25 = 1251.25, 1002 * 1.25 = 1252.5.
        assert_eq!(scaled(1001, 150), 1251);
        assert_eq!(scaled(1002, 150), 1253);
    }
}
