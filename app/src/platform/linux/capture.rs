use std::{
    cell::{Cell, RefCell},
    os::fd::OwnedFd,
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::SyncSender,
        Arc,
    },
    time::{Duration, Instant},
};

use image::GrayImage;

use crate::config::Rect;

pub use crate::platform::RegionFrame;
use crate::platform::{CaptureControl, CaptureEvent};

/// Capture throttle while the bar gate is open (the panel is on screen):
/// effectively compositor rate. Motion tracking runs on every captured
/// frame on a thread of its own (see reward_pipeline), so a scroll arrives
/// as the small steps it is made of; each open-gate frame costs one
/// grayscale crop plus about a millisecond of tracking, and tesseract runs
/// on another thread at its own pace, so the panel-open CPU cost stays a
/// few ms per frame.
const THROTTLE_OPEN_MS: u64 = 16;
/// Capture throttle while the brightness gate is closed: no point spending
/// CPU on frequent frames nothing will OCR.
const THROTTLE_CLOSED_MS: u64 = 120;
/// Full-frame emission cadence for the rumour recognizer (~1.4 Hz). Rumour
/// tooltips are read at 1-2 Hz (pop-in latency is fine there), and a whole
/// 4K grayscale conversion is a few ms, so this stays cheap next to the
/// reward crop that runs every throttle tick.
const FULL_FRAME_MS: u64 = 700;

pub struct CaptureStart {
    pub node_id: u32,
    pub fd: OwnedFd,
    pub new_token: Option<String>,
}

/// Portal half, run on the caller's tokio runtime: returns the pipewire wiring
/// plus a fresh restore token to persist. Identical flow to the milestone-0
/// spike (SourceType::Window | Monitor, CursorMode::Hidden, PersistMode::ExplicitlyRevoked).
pub async fn portal_session(restore_token: Option<&str>) -> anyhow::Result<CaptureStart> {
    use ashpd::desktop::screencast::{CursorMode, Screencast, SourceType};
    use ashpd::desktop::PersistMode;
    let proxy = Screencast::new().await?;
    let session = proxy.create_session().await?;
    proxy
        .select_sources(
            &session,
            CursorMode::Hidden,
            SourceType::Window | SourceType::Monitor,
            false,
            restore_token,
            PersistMode::ExplicitlyRevoked,
        )
        .await?;
    let response = proxy.start(&session, None).await?.response()?;
    let new_token = response.restore_token().map(str::to_string);
    let stream = response
        .streams()
        .first()
        .ok_or_else(|| anyhow::anyhow!("portal returned no streams"))?;
    let node_id = stream.pipe_wire_node_id();
    let fd = proxy.open_pipe_wire_remote(&session).await?;
    // Session must outlive the pipewire stream; leak it for the app's lifetime.
    std::mem::forget(session);
    Ok(CaptureStart {
        node_id,
        fd,
        new_token,
    })
}

/// The pipewire half, blocking; call on a dedicated thread. It sends a
/// grayscale crop of `region` (capture pixels) on every throttle tick, no
/// matter whether the pixels changed: the downstream state machines
/// (PanelGate's consecutive-frame hysteresis, the stabilizer's
/// confirm-2/switch-2 slot logic) need a steady stream of frames to
/// accumulate consecutive reads even while the panel is static, and a
/// content-hash short-circuit that only emits on change starves them of
/// exactly that (a static panel would produce one frame, then silence, and
/// the gate/confirm counters would never advance). The throttle is dynamic:
/// 120ms while `panel_open` reads true (the OCR worker's brightness gate is
/// open, so scans matter for responsiveness), 300ms while it reads false.
/// `panel_open` is the simplest correct way to hand that one bit of state
/// across the capture/OCR thread boundary without adding a second channel:
/// the OCR worker owns the `PanelGate` and stores its state here every
/// pass; this thread only ever reads it. Region updates arrive on
/// `region_rx` and just update where the crop is taken from; there is no
/// forced-rescan mechanism to trigger anymore, since frames always flow.
/// `tx` is a bounded (capacity-1) sender: the OCR worker only ever wants
/// the latest frame, so this thread `try_send`s and drops on `Full` rather
/// than blocking or queuing, making a backlog structurally impossible
/// instead of relying on the receiver to drain one.
///
/// Returns only when the stream is over, always with the reason as `Err`:
/// a capture that ended is reward pricing being off, and the caller has to
/// know. `consume_supervised` re-opens instead.
pub fn consume(
    start: CaptureStart,
    region_rx: std::sync::mpsc::Receiver<Rect>,
    region: Rect,
    tx: SyncSender<RegionFrame>,
    panel_open: Arc<AtomicBool>,
    full_tx: Option<SyncSender<GrayImage>>,
) -> anyhow::Result<()> {
    let control = CaptureControl::default();
    let shared = Rc::new(Shared {
        region_rx,
        region: Cell::new(region),
        tx,
        panel_open,
        full_tx,
        control: control.clone(),
    });
    let why = run_stream(start, &shared);
    control.report(CaptureEvent::Lost(why.clone()));
    Err(anyhow::anyhow!("capture stream ended: {why}"))
}

/// Waits between re-open attempts, in seconds. A window source disappears
/// for a few seconds while the game recreates its window (display-mode
/// switch, renderer restart), so the first retries are quick.
const REOPEN_BACKOFF_S: [u64; 6] = [1, 2, 5, 10, 30, 60];
/// A stream that lived this long was healthy; the next loss starts the
/// backoff from the beginning.
const HEALTHY_RUN: Duration = Duration::from_secs(60);

/// `consume` that survives the stream ending. When the stream errors or
/// its source goes away (a window source when the game recreates its
/// window, a pipewire restart), `reopen` is asked for a fresh portal
/// session - the caller runs `portal_session` with the saved restore token
/// on its runtime - and streaming resumes on the same channels. Every
/// transition is reported through `control.events`. Returns `Err` only
/// after the re-open attempts are exhausted.
///
/// While `control.paused` is set, frames are dequeued and dropped before
/// any pixel work.
#[allow(clippy::too_many_arguments)]
pub fn consume_supervised(
    start: CaptureStart,
    region_rx: std::sync::mpsc::Receiver<Rect>,
    region: Rect,
    tx: SyncSender<RegionFrame>,
    panel_open: Arc<AtomicBool>,
    full_tx: Option<SyncSender<GrayImage>>,
    control: CaptureControl,
    mut reopen: impl FnMut() -> anyhow::Result<CaptureStart>,
) -> anyhow::Result<()> {
    let shared = Rc::new(Shared {
        region_rx,
        region: Cell::new(region),
        tx,
        panel_open,
        full_tx,
        control: control.clone(),
    });
    let mut next = Some(start);
    let mut attempt = 0usize;
    loop {
        if let Some(start) = next.take() {
            let began = Instant::now();
            let why = run_stream(start, &shared);
            control.report(CaptureEvent::Lost(why));
            if began.elapsed() >= HEALTHY_RUN {
                attempt = 0;
            }
        }
        let Some(wait) = REOPEN_BACKOFF_S.get(attempt) else {
            let why = "the screen capture could not be re-opened".to_string();
            control.report(CaptureEvent::GaveUp(why.clone()));
            anyhow::bail!(why);
        };
        attempt += 1;
        std::thread::sleep(Duration::from_secs(*wait));
        match reopen() {
            Ok(start) => {
                if let Some(token) = &start.new_token {
                    control.report(CaptureEvent::NewToken(token.clone()));
                }
                next = Some(start);
            }
            Err(e) => eprintln!("capture: re-open failed: {e}"),
        }
    }
}

/// What one stream's callbacks share with the supervisor, so a re-opened
/// stream continues on the same channels and the same region.
struct Shared {
    region_rx: std::sync::mpsc::Receiver<Rect>,
    region: Cell<Rect>,
    tx: SyncSender<RegionFrame>,
    panel_open: Arc<AtomicBool>,
    full_tx: Option<SyncSender<GrayImage>>,
    control: CaptureControl,
}

/// Grayscale of the `w`x`h` rectangle at (`x0`, `y0`) of a 4-bytes-per-pixel
/// BGRx frame. `None` when the buffer is shorter than the announced
/// geometry needs: during a format renegotiation pipewire can hand over a
/// buffer of the OLD size with the NEW size already parsed, and an
/// unchecked slice there panics inside a C callback, which aborts the
/// whole process.
pub fn gray_crop(bytes: &[u8], stride: usize, x0: usize, y0: usize, w: usize, h: usize) -> Option<Vec<u8>> {
    // Direct writes into a raw buffer via chunks_exact, rather than
    // GrayImage::put_pixel per pixel: put_pixel's per-call bounds check and
    // coordinate math are a real constant factor over a ~1M-pixel crop
    // running every throttle tick.
    let mut raw = vec![0u8; w.checked_mul(h)?];
    let row_bytes = w.checked_mul(4)?;
    for (row, dst_row) in raw.chunks_exact_mut(w.max(1)).enumerate() {
        let base = y0.checked_add(row)?.checked_mul(stride)?.checked_add(x0.checked_mul(4)?)?;
        let src_row = bytes.get(base..base.checked_add(row_bytes)?)?;
        for (dst, px) in dst_row.iter_mut().zip(src_row.chunks_exact(4)) {
            // BGRx
            *dst = (0.114 * px[0] as f32 + 0.587 * px[1] as f32 + 0.299 * px[2] as f32) as u8;
        }
    }
    Some(raw)
}

/// Runs one pipewire stream until it ends and returns why it ended.
fn run_stream(start: CaptureStart, shared: &Rc<Shared>) -> String {
    match run_stream_inner(start, shared) {
        Ok(why) => why,
        Err(e) => e.to_string(),
    }
}

fn run_stream_inner(start: CaptureStart, shared: &Rc<Shared>) -> anyhow::Result<String> {
    use pipewire as pw;
    use pw::{properties::properties, spa};
    use spa::pod::Pod;

    #[derive(Default)]
    struct State {
        format: spa::param::video::VideoInfoRaw,
        last_sent: Option<Instant>,
        last_full: Option<Instant>,
    }

    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&mainloop, None)?;
    let core = context.connect_fd_rc(start.fd, None)?;
    let stream = pw::stream::StreamBox::new(
        &core,
        "khaloni-poe2-capture",
        properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )?;

    // Why the loop was told to quit. Both listeners below end the loop the
    // same way: record the reason, quit, and let the caller decide.
    let ended: Rc<RefCell<Option<String>>> = Rc::new(RefCell::new(None));
    let quit = {
        let ended = ended.clone();
        let weak = Rc::new(mainloop.downgrade());
        move |why: String| {
            ended.borrow_mut().get_or_insert(why);
            if let Some(ml) = weak.upgrade() {
                ml.quit();
            }
        }
    };

    // The connection to pipewire itself failing (daemon restart) never
    // reaches the stream as a state change.
    let _core_listener = core
        .add_listener_local()
        .error({
            let quit = quit.clone();
            move |id, _seq, res, message| {
                if id == pw::core::PW_ID_CORE {
                    quit(format!("pipewire connection error {res}: {message}"));
                }
            }
        })
        .register();

    let process_shared = shared.clone();
    let state_shared = shared.clone();
    let _listener = stream
        .add_local_listener_with_user_data(State::default())
        .state_changed(move |_, _, old, new| {
            use pw::stream::StreamState;
            match new {
                StreamState::Error(msg) => quit(format!("stream error: {msg}")),
                // Paused is normal (a minimized window source); falling back
                // to unconnected after having been connected is the source
                // going away.
                StreamState::Unconnected if !matches!(old, StreamState::Unconnected) => {
                    quit("the captured source went away".to_string())
                }
                StreamState::Streaming => state_shared.control.report(CaptureEvent::Streaming),
                _ => {}
            }
        })
        .param_changed(|_, state, id, param| {
            let Some(param) = param else { return };
            if id != spa::param::ParamType::Format.as_raw() {
                return;
            }
            let _ = state.format.parse(param);
        })
        .process(move |stream, state| {
            let shared = &process_shared;
            while let Ok(r) = shared.region_rx.try_recv() {
                shared.region.set(r);
            }
            let region = shared.region.get();
            // Always dequeue: an un-dequeued buffer never returns to the pool,
            // and a starved pool stalls the stream permanently. Throttling
            // drops the dequeued frame instead of skipping the dequeue.
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            if shared.control.paused.load(Ordering::Relaxed) {
                return;
            }
            if let Some(t) = state.last_sent {
                let throttle_ms = if shared.panel_open.load(Ordering::Relaxed) {
                    THROTTLE_OPEN_MS
                } else {
                    THROTTLE_CLOSED_MS
                };
                if t.elapsed() < Duration::from_millis(throttle_ms) {
                    return;
                }
            }
            let datas = buffer.datas_mut();
            let Some(data) = datas.first_mut() else {
                return;
            };
            let stride = data.chunk().stride() as usize;
            let Some(bytes) = data.data() else { return };
            let (fw, fh) = (state.format.size().width, state.format.size().height);
            if fw == 0 || fh == 0 {
                return;
            }
            let x0 = region.x.clamp(0, fw as i32 - 1) as usize;
            let y0 = region.y.clamp(0, fh as i32 - 1) as usize;
            let w = (region.w as usize).min(fw as usize - x0);
            let h = (region.h as usize).min(fh as usize - y0);
            if w == 0 || h == 0 {
                return;
            }
            let Some(raw) = gray_crop(bytes, stride, x0, y0, w, h) else {
                return;
            };
            let Some(gray) = GrayImage::from_raw(w as u32, h as u32, raw) else {
                return;
            };
            state.last_sent = Some(Instant::now());
            // The OCR worker only ever wants the latest frame: drop this
            // one on a full channel instead of blocking or queuing.
            let _ = shared.tx.try_send(RegionFrame { gray });

            // Full-frame emission for the rumour recognizer, on its own slow
            // cadence. The rumour tooltip can be anywhere on screen, so it
            // needs the whole frame (find_panel scans it) rather than the
            // reward crop. Same latest-only, drop-on-full contract.
            if let Some(ft) = &shared.full_tx {
                let due = state
                    .last_full
                    .is_none_or(|t| t.elapsed() >= Duration::from_millis(FULL_FRAME_MS));
                if due {
                    let full = gray_crop(bytes, stride, 0, 0, fw as usize, fh as usize)
                        .and_then(|fraw| GrayImage::from_raw(fw, fh, fraw));
                    if let Some(full) = full {
                        if ft.try_send(full).is_ok() {
                            state.last_full = Some(Instant::now());
                        }
                    }
                }
            }
        })
        .register()?;

    let obj = spa::pod::object!(
        spa::utils::SpaTypes::ObjectParamFormat,
        spa::param::ParamType::EnumFormat,
        spa::pod::property!(
            spa::param::format::FormatProperties::MediaType,
            Id,
            spa::param::format::MediaType::Video
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::MediaSubtype,
            Id,
            spa::param::format::MediaSubtype::Raw
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            spa::param::video::VideoFormat::BGRx,
            spa::param::video::VideoFormat::BGRx,
            spa::param::video::VideoFormat::BGRA,
            spa::param::video::VideoFormat::RGBx,
            spa::param::video::VideoFormat::RGBA
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            spa::utils::Rectangle { width: 3840, height: 2160 },
            spa::utils::Rectangle { width: 1, height: 1 },
            spa::utils::Rectangle { width: 8192, height: 8192 }
        ),
        spa::pod::property!(
            spa::param::format::FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            spa::utils::Fraction { num: 30, denom: 1 },
            spa::utils::Fraction { num: 0, denom: 1 },
            spa::utils::Fraction { num: 1000, denom: 1 }
        ),
    );
    let values = spa::pod::serialize::PodSerializer::serialize(
        std::io::Cursor::new(Vec::new()),
        &spa::pod::Value::Object(obj),
    )?
    .0
    .into_inner();
    let mut params = [Pod::from_bytes(&values).ok_or_else(|| anyhow::anyhow!("bad pod"))?];
    stream.connect(
        spa::utils::Direction::Input,
        Some(start.node_id),
        pw::stream::StreamFlags::AUTOCONNECT | pw::stream::StreamFlags::MAP_BUFFERS,
        &mut params,
    )?;
    mainloop.run();
    let why = ended.borrow_mut().take();
    Ok(why.unwrap_or_else(|| "the pipewire loop stopped".to_string()))
}

#[cfg(test)]
mod tests {
    use super::gray_crop;

    #[test]
    fn crop_converts_bgrx_with_stride_and_offset() {
        // 3x2 frame, stride 16 (one padding pixel per row).
        let mut frame = vec![0u8; 32];
        // Pixel (1,1) pure green, pixel (2,1) pure white.
        frame[16 + 4..16 + 8].copy_from_slice(&[0, 255, 0, 0]);
        frame[16 + 8..16 + 12].copy_from_slice(&[255, 255, 255, 0]);
        let raw = gray_crop(&frame, 16, 1, 1, 2, 1).unwrap();
        assert_eq!(raw, vec![(0.587f32 * 255.0) as u8, 255]);
    }

    #[test]
    fn a_buffer_shorter_than_the_announced_frame_is_skipped_not_a_panic() {
        // Format says 4x4 but the buffer still holds a 2x2 frame.
        let small = vec![0u8; 2 * 2 * 4];
        assert_eq!(gray_crop(&small, 16, 0, 0, 4, 4), None);
        assert_eq!(gray_crop(&small, 8, 1, 1, 2, 2), None);
        assert_eq!(gray_crop(&[], 0, 0, 0, 1, 1), None);
        assert_eq!(gray_crop(&small, usize::MAX, 1, 1, 1, 1), None);
        // The part that fits still converts.
        assert!(gray_crop(&small, 8, 0, 0, 2, 2).is_some());
    }
}
