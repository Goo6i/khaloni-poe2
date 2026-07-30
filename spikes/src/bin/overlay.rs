use std::{
    process::Command,
    sync::mpsc::{channel, Sender},
};

use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState, Region},
    delegate_compositor, delegate_layer, delegate_output, delegate_registry, delegate_shm,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::{
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
        WaylandSurface,
    },
    shm::{slot::SlotPool, Shm, ShmHandler},
};
use wayland_client::{
    globals::registry_queue_init,
    protocol::{wl_output, wl_shm, wl_surface},
    Connection, QueueHandle,
};

const PANEL_W: u32 = 420;
const PANEL_H: u32 = 180;
// Where the panel sits relative to the tracked window's top-left corner.
const OFFSET_X: i32 = 200;
const OFFSET_Y: i32 = 200;

const KWIN_SCRIPT: &str = r#"
function matches(w) {
    return (w.caption + " " + w.resourceClass).toLowerCase().includes("@SUBSTR@");
}
function report(w) {
    callDBus("org.khalonipoe2.Spike", "/org/khalonipoe2/Spike", "org.khalonipoe2.Spike", "Geometry",
             Math.round(w.frameGeometry.x), Math.round(w.frameGeometry.y),
             Math.round(w.frameGeometry.width), Math.round(w.frameGeometry.height));
}
function hook(w) {
    if (!matches(w)) return;
    report(w);
    w.frameGeometryChanged.connect(function () { report(w); });
}
for (const w of workspace.windowList()) hook(w);
workspace.windowAdded.connect(hook);
"#;

struct SpikeService {
    tx: Sender<(i32, i32, u32, u32)>,
}

#[zbus::interface(name = "org.khalonipoe2.Spike")]
impl SpikeService {
    fn geometry(&self, x: i32, y: i32, w: i32, h: i32) {
        let _ = self.tx.send((x, y, w as u32, h as u32));
    }
}

fn qdbus(args: &[&str]) -> Option<String> {
    let out = Command::new("qdbus6").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn load_kwin_script(substr: &str) -> Option<String> {
    let path = std::env::temp_dir().join("khalonipoe2-spike-kwin.js");
    std::fs::write(&path, KWIN_SCRIPT.replace("@SUBSTR@", &substr.to_lowercase())).ok()?;
    let id = qdbus(&[
        "org.kde.KWin",
        "/Scripting",
        "org.kde.kwin.Scripting.loadScript",
        path.to_str()?,
    ])?;
    let obj = format!("/Scripting/Script{id}");
    if qdbus(&["org.kde.KWin", &obj, "org.kde.kwin.Script.run"]).is_some() {
        return Some(obj);
    }
    let alt = format!("/{id}");
    qdbus(&["org.kde.KWin", &alt, "org.kde.kwin.Script.run"]).map(|_| alt)
}

fn stop_kwin_script(obj: &str) {
    qdbus(&["org.kde.KWin", obj, "org.kde.kwin.Script.stop"]);
}

struct App {
    registry_state: RegistryState,
    output_state: OutputState,
    shm: Shm,
    pool: SlotPool,
    layer: Option<LayerSurface>,
    surface_size: (u32, u32),
    panel_pos: (i32, i32),
    exit: bool,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let want = std::env::args().nth(1).unwrap_or_else(|| "path of exile".into());

    // DBus service the KWin script reports window geometry to.
    let (tx, rx) = channel();
    let _dbus = zbus::blocking::connection::Builder::session()?
        .name("org.khalonipoe2.Spike")?
        .serve_at("/org/khalonipoe2/Spike", SpikeService { tx })?
        .build()?;

    let script = load_kwin_script(&want).ok_or("failed to load KWin script")?;
    eprintln!("kwin script loaded at {script}, waiting for first geometry of {want:?}");
    let (mut gx, mut gy, gw, gh) = rx.recv_timeout(std::time::Duration::from_secs(
        std::env::var("SPIKE_WAIT").ok().and_then(|v| v.parse().ok()).unwrap_or(10),
    ))?;
    eprintln!("tracked window: {gx},{gy} {gw}x{gh}");

    let conn = Connection::connect_to_env()?;
    let (globals, mut event_queue) = registry_queue_init(&conn)?;
    let qh: QueueHandle<App> = event_queue.handle();

    let compositor = CompositorState::bind(&globals, &qh)?;
    let layer_shell = LayerShell::bind(&globals, &qh)?;
    let shm = Shm::bind(&globals, &qh)?;

    let pool = SlotPool::new(1024 * 1024, &shm)?;
    let mut app = App {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        shm,
        pool,
        layer: None,
        surface_size: (0, 0),
        panel_pos: (OFFSET_X, OFFSET_Y),
        exit: false,
    };

    // Two roundtrips so OutputState is populated, then pick the output containing
    // the tracked window's center.
    event_queue.roundtrip(&mut app)?;
    event_queue.roundtrip(&mut app)?;
    let (cx, cy) = (gx + gw as i32 / 2, gy + gh as i32 / 2);
    let mut target = None;
    let mut output_pos = (0i32, 0i32);
    for output in app.output_state.outputs() {
        if let Some(info) = app.output_state.info(&output) {
            let pos = info.logical_position.unwrap_or_default();
            let size = info.logical_size.unwrap_or_default();
            let name = info.name.clone().unwrap_or_default();
            eprintln!("output {name}: pos={pos:?} size={size:?}");
            if cx >= pos.0 && cx < pos.0 + size.0 && cy >= pos.1 && cy < pos.1 + size.1 {
                output_pos = (pos.0, pos.1);
                target = Some(output);
                eprintln!("-> window center is on {name}");
            }
        }
    }
    if target.is_none() {
        eprintln!("no output contains the window center; compositor picks");
    }

    let surface = compositor.create_surface(&qh);
    let layer = layer_shell.create_layer_surface(
        &qh,
        surface,
        Layer::Overlay,
        Some("khaloni-poe2-spike"),
        target.as_ref(),
    );
    // Cover the whole output; panel position is drawn inside the canvas.
    layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
    layer.set_exclusive_zone(-1);
    layer.set_keyboard_interactivity(KeyboardInteractivity::None);

    // Empty input region = every click falls through to whatever is beneath.
    let region = Region::new(&compositor)?;
    layer.wl_surface().set_input_region(Some(region.wl_region()));
    layer.commit();
    app.layer = Some(layer);
    app.panel_pos = (gx - output_pos.0 + OFFSET_X, gy - output_pos.1 + OFFSET_Y);

    eprintln!("overlay up for 120s, following the tracked window");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    while !app.exit && std::time::Instant::now() < deadline {
        let mut moved = false;
        while let Ok((nx, ny, _, _)) = rx.try_recv() {
            (gx, gy) = (nx, ny);
            moved = true;
        }
        if moved {
            app.panel_pos = (gx - output_pos.0 + OFFSET_X, gy - output_pos.1 + OFFSET_Y);
            eprintln!("window moved: panel -> {:?}", app.panel_pos);
            app.draw();
        }
        event_queue.roundtrip(&mut app)?;
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    stop_kwin_script(&script);
    Ok(())
}

impl App {
    fn draw(&mut self) {
        let Some(layer) = self.layer.clone() else { return };
        let (w, h) = self.surface_size;
        if w == 0 || h == 0 {
            return;
        }
        let stride = (w * 4) as i32;
        let (buffer, canvas) = self
            .pool
            .create_buffer(w as i32, h as i32, stride, wl_shm::Format::Argb8888)
            .expect("create buffer");
        canvas.fill(0); // fully transparent
        let (px, py) = self.panel_pos;
        for row in 0..PANEL_H as i32 {
            let y = py + row;
            if y < 0 || y >= h as i32 {
                continue;
            }
            for col in 0..PANEL_W as i32 {
                let x = px + col;
                if x < 0 || x >= w as i32 {
                    continue;
                }
                let i = ((y * w as i32 + x) * 4) as usize;
                // Premultiplied ARGB little-endian bytes: [B, G, R, A]. 55% translucent green.
                canvas[i..i + 4].copy_from_slice(&[0x20, 0x8c, 0x20, 0x8c]);
            }
        }
        layer.wl_surface().damage_buffer(0, 0, w as i32, h as i32);
        buffer.attach_to(layer.wl_surface()).expect("attach buffer");
        layer.commit();
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
        eprintln!("layer configured at {:?}", self.surface_size);
        self.draw();
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

impl ShmHandler for App {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState];
}

delegate_compositor!(App);
delegate_output!(App);
delegate_shm!(App);
delegate_layer!(App);
delegate_registry!(App);
