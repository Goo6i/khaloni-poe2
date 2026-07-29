//! Windows screen-capture backend: Windows Graphics Capture (WGC) via the
//! windows-capture crate, behind the same public surface as
//! platform/linux/capture.rs (portal_session + consume).
//!
//! Windows-specific contracts and divergences from the Linux twin:
//!
//! - **No portal, no restore token.** On Linux the XDG desktop portal owns
//!   source selection and hands back a restore token to skip the picker next
//!   run. On Windows the target is chosen programmatically in
//!   [`portal_session`]: the tracked game window (from
//!   `crate::platform::gamewin::game_hwnd()`) when one exists, else the
//!   primary monitor. `new_token` is therefore always `None` and the
//!   `restore_token` argument is ignored; main.rs's persist-the-token step
//!   becomes a no-op.
//! - **`CaptureStart` is per-platform by design.** The Linux one carries
//!   pipewire wiring (node id + fd); this one carries the capture-target
//!   choice made in `portal_session`, consumed by [`consume`].
//! - **Same frame semantics.** BGRA frames (stride-respecting) are converted
//!   with the identical luma weights as the Linux BGRx path, cropped to the
//!   region in capture pixels, and emitted under the same throttle contract:
//!   [`THROTTLE_OPEN_MS`] while the OCR worker's brightness gate reads open,
//!   [`THROTTLE_CLOSED_MS`] otherwise, plus full frames on `full_tx` every
//!   [`FULL_FRAME_MS`]. Region updates arrive on `region_rx`, and `tx` keeps
//!   the latest-only `try_send`-and-drop contract.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{Receiver, SyncSender},
    Arc,
};
use std::time::{Duration, Instant};

use image::GrayImage;

use crate::config::Rect;

pub use crate::platform::RegionFrame;

/// Capture throttle while the brightness gate is open (the panel is
/// probably on screen): effectively compositor rate. Same rationale as the
/// Linux twin: motion tracking needs per-frame cadence while the panel is
/// up; the heavy OCR paths are cadence-gated downstream.
const THROTTLE_OPEN_MS: u64 = 16;
/// Capture throttle while the brightness gate is closed: no point spending
/// CPU on frequent frames nothing will OCR.
const THROTTLE_CLOSED_MS: u64 = 120;
/// Full-frame emission cadence for the rumour recognizer (~1.4 Hz).
const FULL_FRAME_MS: u64 = 700;

/// What WGC should capture, decided once in [`portal_session`].
pub enum CaptureTarget {
    /// The game window, by raw HWND (from the gamewin tracker).
    Window(isize),
    /// Fallback when no game window is tracked yet.
    PrimaryMonitor,
}

/// The Windows counterpart of the Linux `CaptureStart` (which carries
/// pipewire wiring): here it carries the WGC target choice. `new_token`
/// exists so main.rs's token-persistence step compiles unchanged; it is
/// always `None` on Windows (WGC has no portal restore tokens).
pub struct CaptureStart {
    pub target: CaptureTarget,
    pub new_token: Option<String>,
}

/// Target selection, mirroring the Linux portal half's role without a
/// portal: prefer the tracked game window from the gamewin feed, fall back
/// to the primary monitor. The `restore_token` argument is accepted for
/// API parity and ignored.
pub async fn portal_session(_restore_token: Option<&str>) -> anyhow::Result<CaptureStart> {
    let target = match crate::platform::gamewin::game_hwnd() {
        Some(hwnd)
            if windows_capture::window::Window::from_raw_hwnd(hwnd as *mut std::ffi::c_void)
                .is_valid() =>
        {
            CaptureTarget::Window(hwnd)
        }
        _ => CaptureTarget::PrimaryMonitor,
    };
    Ok(CaptureStart {
        target,
        new_token: None,
    })
}

/// Everything the frame handler needs, passed through windows-capture's
/// `Settings` flags into [`Handler::new`] on the capture thread.
struct ConsumeState {
    region_rx: Receiver<Rect>,
    region: Rect,
    tx: SyncSender<RegionFrame>,
    panel_open: Arc<AtomicBool>,
    full_tx: Option<SyncSender<GrayImage>>,
    last_sent: Option<Instant>,
    last_full: Option<Instant>,
}

struct Handler {
    st: ConsumeState,
}

/// BGRA row → gray row with the exact luma weights the Linux BGRx path
/// uses (px[0]=B, px[1]=G, px[2]=R; alpha ignored).
fn bgra_to_gray_row(src_row: &[u8], dst_row: &mut [u8]) {
    for (dst, px) in dst_row.iter_mut().zip(src_row.chunks_exact(4)) {
        *dst = (0.114 * px[0] as f32 + 0.587 * px[1] as f32 + 0.299 * px[2] as f32) as u8;
    }
}

impl windows_capture::capture::GraphicsCaptureApiHandler for Handler {
    type Flags = ConsumeState;
    type Error = Box<dyn std::error::Error + Send + Sync>;

    fn new(ctx: windows_capture::capture::Context<Self::Flags>) -> Result<Self, Self::Error> {
        Ok(Handler { st: ctx.flags })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut windows_capture::frame::Frame,
        _capture_control: windows_capture::graphics_capture_api::InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        let st = &mut self.st;
        while let Ok(r) = st.region_rx.try_recv() {
            st.region = r;
        }
        // Throttle before touching the frame: frame.buffer() is a GPU
        // staging copy + map, so skipped ticks must not pay for it. Same
        // steady-stream contract as Linux otherwise: a frame is sent on
        // every due tick whether or not the pixels changed, because the
        // downstream gate/stabilizer counters need consecutive reads.
        if let Some(t) = st.last_sent {
            let throttle_ms = if st.panel_open.load(Ordering::Relaxed) {
                THROTTLE_OPEN_MS
            } else {
                THROTTLE_CLOSED_MS
            };
            if t.elapsed() < Duration::from_millis(throttle_ms) {
                return Ok(());
            }
        }
        let mut fb = frame.buffer()?;
        let (fw, fh) = (fb.width(), fb.height());
        if fw == 0 || fh == 0 {
            return Ok(());
        }
        // Row pitch is the real stride: WGC staging textures pad rows.
        let stride = fb.row_pitch() as usize;
        let bytes: &[u8] = fb.as_raw_buffer();

        let region = st.region;
        let x0 = region.x.clamp(0, fw as i32 - 1) as usize;
        let y0 = region.y.clamp(0, fh as i32 - 1) as usize;
        let w = (region.w as usize).min(fw as usize - x0);
        let h = (region.h as usize).min(fh as usize - y0);
        if w == 0 || h == 0 {
            return Ok(());
        }
        let mut raw = vec![0u8; w * h];
        for row in 0..h {
            let base = (y0 + row) * stride + x0 * 4;
            bgra_to_gray_row(&bytes[base..base + w * 4], &mut raw[row * w..(row + 1) * w]);
        }
        let Some(gray) = GrayImage::from_raw(w as u32, h as u32, raw) else {
            return Ok(());
        };
        st.last_sent = Some(Instant::now());
        // The OCR worker only ever wants the latest frame: drop this one on
        // a full channel instead of blocking or queuing.
        let _ = st.tx.try_send(RegionFrame { gray });

        // Full-frame emission for the rumour recognizer, on its own slow
        // cadence. Same latest-only, drop-on-full contract.
        if let Some(ft) = &st.full_tx {
            let due = st
                .last_full
                .is_none_or(|t| t.elapsed() >= Duration::from_millis(FULL_FRAME_MS));
            if due {
                let (fw_u, fh_u) = (fw as usize, fh as usize);
                let mut fraw = vec![0u8; fw_u * fh_u];
                for row in 0..fh_u {
                    let base = row * stride;
                    bgra_to_gray_row(
                        &bytes[base..base + fw_u * 4],
                        &mut fraw[row * fw_u..(row + 1) * fw_u],
                    );
                }
                if let Some(full) = GrayImage::from_raw(fw, fh, fraw) {
                    if ft.try_send(full).is_ok() {
                        st.last_full = Some(Instant::now());
                    }
                }
            }
        }
        Ok(())
    }
}

/// The WGC half, blocking; call on a dedicated thread (it runs a Win32
/// message loop until the capture session ends, e.g. the captured window
/// closes). Contract identical to the Linux pipewire `consume`: grayscale
/// crops of `region` (capture pixels) on every throttle tick regardless of
/// pixel change, dynamic throttle driven by `panel_open`, region updates
/// honored from `region_rx`, latest-only bounded send on `tx`, and full
/// frames on `full_tx` at [`FULL_FRAME_MS`] for the rumour worker.
pub fn consume(
    start: CaptureStart,
    region_rx: Receiver<Rect>,
    region: Rect,
    tx: SyncSender<RegionFrame>,
    panel_open: Arc<AtomicBool>,
    full_tx: Option<SyncSender<GrayImage>>,
) -> anyhow::Result<()> {
    use windows_capture::capture::GraphicsCaptureApiHandler;
    use windows_capture::settings::{
        ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
        MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
    };

    let st = ConsumeState {
        region_rx,
        region,
        tx,
        panel_open,
        full_tx,
        last_sent: None,
        last_full: None,
    };

    macro_rules! settings_for {
        ($item:expr) => {
            Settings::new(
                $item,
                // Match the Linux portal's CursorMode::Hidden.
                CursorCaptureSettings::WithoutCursor,
                // No yellow capture border around the game window.
                DrawBorderSettings::WithoutBorder,
                SecondaryWindowSettings::Default,
                // Default is ~60 Hz; the real cadence control is our own
                // throttle in the handler, same as the Linux side.
                MinimumUpdateIntervalSettings::Default,
                DirtyRegionSettings::Default,
                ColorFormat::Bgra8,
                st,
            )
        };
    }

    match start.target {
        CaptureTarget::Window(hwnd) => {
            let window =
                windows_capture::window::Window::from_raw_hwnd(hwnd as *mut std::ffi::c_void);
            Handler::start(settings_for!(window))
                .map_err(|e| anyhow::anyhow!("window capture failed: {e}"))
        }
        CaptureTarget::PrimaryMonitor => {
            let monitor = windows_capture::monitor::Monitor::primary()
                .map_err(|e| anyhow::anyhow!("no primary monitor: {e}"))?;
            Handler::start(settings_for!(monitor))
                .map_err(|e| anyhow::anyhow!("monitor capture failed: {e}"))
        }
    }
}
