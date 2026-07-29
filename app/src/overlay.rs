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
use tiny_skia::Pixmap;
use wayland_client::{
    globals::registry_queue_init,
    protocol::{wl_keyboard, wl_output, wl_pointer, wl_seat, wl_shm, wl_surface},
    Connection, EventQueue, QueueHandle,
};

/// A keyboard input relevant to editing a value box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Digit(char),
    Dot,
    Backspace,
    Enter,
    Escape,
}

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
    exit: bool,
}

pub struct Overlay {
    _conn: Connection,
    event_queue: EventQueue<App>,
    app: App,
    output_pos: (i32, i32),
    compositor: CompositorState,
}

impl Overlay {
    pub fn new(target_center: (i32, i32)) -> anyhow::Result<Overlay> {
        let conn = Connection::connect_to_env()?;
        let (globals, event_queue) = registry_queue_init(&conn)?;
        let qh: QueueHandle<App> = event_queue.handle();

        let compositor = CompositorState::bind(&globals, &qh)?;
        let layer_shell = LayerShell::bind(&globals, &qh)?;
        let shm = Shm::bind(&globals, &qh)?;

        let pool = SlotPool::new(1024 * 1024, &shm)?;
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
            exit: false,
        };

        let mut event_queue = event_queue;

        // Two roundtrips so OutputState is populated, then pick the output containing
        // the tracked window's center.
        event_queue.roundtrip(&mut app)?;
        event_queue.roundtrip(&mut app)?;

        let (cx, cy) = target_center;
        let mut target = None;
        let mut output_pos = (0i32, 0i32);
        for output in app.output_state.outputs() {
            if let Some(info) = app.output_state.info(&output) {
                let pos = info.logical_position.unwrap_or_default();
                let size = info.logical_size.unwrap_or_default();
                if cx >= pos.0 && cx < pos.0 + size.0 && cy >= pos.1 && cy < pos.1 + size.1 {
                    output_pos = (pos.0, pos.1);
                    target = Some(output);
                }
            }
        }

        let surface = compositor.create_surface(&qh);
        let layer = layer_shell.create_layer_surface(
            &qh,
            surface,
            Layer::Overlay,
            Some("poe2-lens"),
            target.as_ref(),
        );
        // Cover the whole output.
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);

        // Empty input region = every click falls through to whatever is beneath.
        let region = Region::new(&compositor)?;
        layer.wl_surface().set_input_region(Some(region.wl_region()));
        layer.commit();
        app.layer = Some(layer);

        Ok(Overlay {
            _conn: conn,
            event_queue,
            app,
            output_pos,
            compositor,
        })
    }

    /// Makes `rect` (surface-local logical px) accept pointer input, or
    /// restores full click-through with None. The wl_region contents are
    /// copied by set_input_region, so the Region can drop right after.
    pub fn set_interactive(&mut self, rect: Option<(i32, i32, u32, u32)>) -> anyhow::Result<()> {
        let layer = self.app.layer.as_ref().expect("layer created in new()");
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

    /// Drains editing keystrokes received since the last call.
    pub fn take_keys(&mut self) -> Vec<Key> {
        std::mem::take(&mut self.app.keys)
    }

    /// Requests (or releases) keyboard focus for the overlay surface, so a
    /// focused value box can receive typed digits. On-demand: the compositor
    /// grants focus on the pointer interaction that opened the box.
    pub fn set_keyboard(&mut self, on: bool) -> anyhow::Result<()> {
        let layer = self.app.layer.as_ref().expect("layer created in new()");
        layer.set_keyboard_interactivity(if on {
            KeyboardInteractivity::OnDemand
        } else {
            KeyboardInteractivity::None
        });
        layer.commit();
        if !on {
            self.app.keys.clear();
        }
        Ok(())
    }

    pub fn pump(&mut self) -> anyhow::Result<()> {
        self.event_queue.roundtrip(&mut self.app)?;
        Ok(())
    }

    pub fn present(&mut self, pixmap: &Pixmap) -> anyhow::Result<()> {
        let (w, h) = self.app.surface_size;
        if w == 0 || h == 0 {
            return Ok(());
        }
        let stride = (w * 4) as i32;
        let (buffer, canvas) = self
            .app
            .pool
            .create_buffer(w as i32, h as i32, stride, wl_shm::Format::Argb8888)?;
        let src = pixmap.data();
        // tiny-skia stores premultiplied RGBA; wl ARGB8888 little-endian wants [B,G,R,A].
        for (dst, s) in canvas.chunks_exact_mut(4).zip(src.chunks_exact(4)) {
            dst[0] = s[2];
            dst[1] = s[1];
            dst[2] = s[0];
            dst[3] = s[3];
        }
        let layer = self.app.layer.as_ref().expect("layer created in new()");
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
}

impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &LayerSurface) {
        self.exit = true;
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
    fn scale_factor_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: i32) {}
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
                    }
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
        _: KeyEvent,
    ) {
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

delegate_compositor!(App);
delegate_output!(App);
delegate_shm!(App);
delegate_seat!(App);
delegate_keyboard!(App);
delegate_pointer!(App);
delegate_layer!(App);
delegate_registry!(App);
