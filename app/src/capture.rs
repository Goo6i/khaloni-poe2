use std::{
    os::fd::OwnedFd,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::SyncSender,
        Arc,
    },
    time::{Duration, Instant},
};

use image::GrayImage;

use crate::config::Rect;

/// Capture throttle while the brightness gate is open (the panel is
/// probably on screen): scan more often for responsiveness.
const THROTTLE_OPEN_MS: u64 = 120;
/// Capture throttle while the brightness gate is closed: no point spending
/// CPU on frequent frames nothing will OCR.
const THROTTLE_CLOSED_MS: u64 = 120;

pub struct RegionFrame {
    pub gray: GrayImage,
}

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
/// (BrightnessGate's consecutive-frame hysteresis, the stabilizer's
/// confirm-2/switch-2 slot logic) need a steady stream of frames to
/// accumulate consecutive reads even while the panel is static, and a
/// content-hash short-circuit that only emits on change starves them of
/// exactly that (a static panel would produce one frame, then silence, and
/// the gate/confirm counters would never advance). The throttle is dynamic:
/// 120ms while `panel_open` reads true (the OCR worker's brightness gate is
/// open, so scans matter for responsiveness), 300ms while it reads false.
/// `panel_open` is the simplest correct way to hand that one bit of state
/// across the capture/OCR thread boundary without adding a second channel:
/// the OCR worker owns the `BrightnessGate` and stores its state here every
/// pass; this thread only ever reads it. Region updates arrive on
/// `region_rx` and just update where the crop is taken from; there is no
/// forced-rescan mechanism to trigger anymore, since frames always flow.
/// `tx` is a bounded (capacity-1) sender: the OCR worker only ever wants
/// the latest frame, so this thread `try_send`s and drops on `Full` rather
/// than blocking or queuing, making a backlog structurally impossible
/// instead of relying on the receiver to drain one.
pub fn consume(
    start: CaptureStart,
    region_rx: std::sync::mpsc::Receiver<Rect>,
    mut region: Rect,
    tx: SyncSender<RegionFrame>,
    panel_open: Arc<AtomicBool>,
) -> anyhow::Result<()> {
    use pipewire as pw;
    use pw::{properties::properties, spa};
    use spa::pod::Pod;

    #[derive(Default)]
    struct State {
        format: spa::param::video::VideoInfoRaw,
        last_sent: Option<Instant>,
    }

    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None)?;
    let context = pw::context::ContextRc::new(&mainloop, None)?;
    let core = context.connect_fd_rc(start.fd, None)?;
    let stream = pw::stream::StreamBox::new(
        &core,
        "poe2-lens-capture",
        properties! {
            *pw::keys::MEDIA_TYPE => "Video",
            *pw::keys::MEDIA_CATEGORY => "Capture",
            *pw::keys::MEDIA_ROLE => "Screen",
        },
    )?;

    let _listener = stream
        .add_local_listener_with_user_data(State::default())
        .param_changed(|_, state, id, param| {
            let Some(param) = param else { return };
            if id != spa::param::ParamType::Format.as_raw() {
                return;
            }
            let _ = state.format.parse(param);
        })
        .process(move |stream, state| {
            while let Ok(r) = region_rx.try_recv() {
                region = r;
            }
            // Always dequeue: an un-dequeued buffer never returns to the pool,
            // and a starved pool stalls the stream permanently. Throttling
            // drops the dequeued frame instead of skipping the dequeue.
            let Some(mut buffer) = stream.dequeue_buffer() else {
                return;
            };
            if let Some(t) = state.last_sent {
                let throttle_ms = if panel_open.load(Ordering::Relaxed) {
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
            // Direct writes into a raw buffer via chunks_exact, rather than
            // GrayImage::put_pixel per pixel: put_pixel's per-call bounds
            // check and coordinate math are a real constant factor over a
            // ~1M-pixel crop running every throttle tick.
            let mut raw = vec![0u8; w * h];
            for row in 0..h {
                let base = (y0 + row) * stride + x0 * 4;
                let src_row = &bytes[base..base + w * 4];
                let dst_row = &mut raw[row * w..(row + 1) * w];
                for (dst, px) in dst_row.iter_mut().zip(src_row.chunks_exact(4)) {
                    // BGRx
                    *dst = (0.114 * px[0] as f32 + 0.587 * px[1] as f32 + 0.299 * px[2] as f32) as u8;
                }
            }
            let Some(gray) = GrayImage::from_raw(w as u32, h as u32, raw) else {
                return;
            };
            state.last_sent = Some(Instant::now());
            // The OCR worker only ever wants the latest frame: drop this
            // one on a full channel instead of blocking or queuing.
            let _ = tx.try_send(RegionFrame { gray });
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
    Ok(())
}
