//! The reward-row pipeline: frames of the reward panel in, stabilizer
//! messages out.
//!
//! Two threads. The tracking thread takes every captured frame (16 ms
//! while the panel is open) and runs everything that costs about a
//! millisecond: the row signature and the list's motion, the bar gate,
//! band detection and template hits. Tesseract takes hundreds of
//! milliseconds a pass, so it runs on the reading thread, which always
//! reads the newest frame handed to it; a pass that is still running when
//! newer frames arrive never stops the tracking. When both shared a thread,
//! every frame during a read was dropped and a scroll reached the tracker
//! as one large jump.
//!
//! Every read is tagged with the scroll position of the frame it was made
//! from (a `ScrollMark`), so the stabilizer can move its rows by the scroll
//! that happened while tesseract ran, or drop the ones that left the list.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use image::GrayImage;

use crate::gate::PanelGate;
use crate::ocr::{self, Motion, RowSignature, UiScale, UPSCALE};
use crate::pricing::Priced;
use crate::stabilize::{Scan, ScanResult, ScrollMark};
use crate::template::VerifyTicket;

/// One captured frame of the reward region, with the UI scale of the whole
/// game frame it was cut from.
pub struct Frame {
    pub gray: GrayImage,
    pub scale: UiScale,
}

/// What the pipeline sends the main loop: a stabilizer message, with the
/// league its prices were looked up in (None for messages without prices).
pub type Message = (Option<String>, ScanResult);

/// The template store's answer for one frame's bars.
#[derive(Default)]
pub struct TemplatePass {
    /// Rows identified from learned strips, positioned on their bars.
    pub rows: Vec<Priced>,
    /// Bars (by top y) whose hit still owes an OCR confirmation; their rows
    /// are in `rows` but are shown only once a read has agreed.
    pub tickets: Vec<(u32, VerifyTicket)>,
    /// Some bar had no usable hit and needs tesseract.
    pub unresolved: bool,
    pub league: Option<String>,
    pub stale: bool,
}

/// Template lookup, run on the tracking thread for every frame with bars.
pub trait Resolve {
    fn resolve(&mut self, frame: &Frame, bars: &[(u32, u32)]) -> TemplatePass;
}

/// A frame handed to the reading thread.
pub struct OcrJob {
    pub frame: Arc<Frame>,
    pub bars: Vec<(u32, u32)>,
    /// False on the first read after the list moved: bands only, without
    /// the whole-panel pass, so revealed rows come back sooner.
    pub with_whole: bool,
    pub templates: TemplatePass,
    /// Where the list stood in this frame.
    pub at: ScrollMark,
}

/// A finished read: every row priced on the job's frame.
pub struct ReadOut {
    pub rows: Vec<Priced>,
    pub league: Option<String>,
    pub stale: bool,
}

/// Tesseract and pricing, run on the reading thread.
pub trait ReadRows {
    /// None when nothing could be read at all (no message is sent).
    fn read(&mut self, job: &OcrJob) -> Option<ReadOut>;
}

/// The message for a finished read, tagged with its frame's position.
pub fn finish(job: &OcrJob, out: ReadOut) -> Message {
    let scan = Scan { rows: out.rows, stale: out.stale, at: Some(job.at), partial: false };
    (out.league, ScanResult::Rows(scan))
}

/// Tesseract cadence floor while the list is still: a panel whose rows
/// the templates cannot all resolve is re-read at most this often. The
/// first still frame after the list moved is read at once.
const HEAVY_EVERY: Duration = Duration::from_millis(120);

/// The tracking thread's state, driven one frame at a time.
#[derive(Default)]
pub struct Tracker {
    /// The previous frame's signature, size and scale.
    prev: Option<(RowSignature, (u32, u32), UiScale)>,
    /// Rows the scrolling list occupies: the union of every reward bar
    /// seen since the gate opened. Bars are only ever drawn inside the
    /// list's viewport, so this never reaches the static header or footer.
    span: Option<(u32, u32)>,
    gate: PanelGate,
    mark: ScrollMark,
    /// The list moved since the last read was requested.
    moved: bool,
    last_job: Option<Instant>,
}

/// What one frame produced.
pub struct Step {
    /// The bar gate's state after this frame.
    pub open: bool,
    pub motion: Motion,
    pub bars: usize,
    /// Messages for the stabilizer, in order.
    pub messages: Vec<Message>,
    /// A read to hand to the reading thread, replacing any not yet started.
    pub job: Option<OcrJob>,
}

impl Tracker {
    pub fn new() -> Tracker {
        Tracker::default()
    }

    pub fn step(&mut self, frame: Frame, now: Instant, resolver: &mut dyn Resolve) -> Step {
        let scale = frame.scale;
        let dims = frame.gray.dimensions();
        let sig = RowSignature::of(&frame.gray);
        // Presence first: the signature bars in the region are the only
        // evidence of a reward panel (tests/bars.rs).
        let bars = ocr::reward_bars_at(&frame.gray, sig.profile(), scale);
        let motion = match (&self.prev, self.span) {
            (None, _) => Motion::Still,
            // A new region or client size: nothing carries over.
            (Some((_, d, s)), _) if *d != dims || *s != scale => {
                self.span = None;
                Motion::Lost
            }
            (Some((prev, ..)), Some(span)) => ocr::track_motion(prev, &sig, span, scale),
            (Some(_), None) => Motion::Still,
        };
        self.prev = Some((sig, dims, scale));
        // A moving list holds the gate rather than feeding it: the bars
        // at its edges come and go as rows scroll through.
        let open = match motion {
            Motion::Still => self.gate.observe(!bars.is_empty()),
            _ => self.gate.is_open(),
        };
        let mut step = Step { open, motion, bars: bars.len(), messages: Vec::new(), job: None };
        if !open {
            // No reward rows on screen: no tesseract, and the overlay drops
            // what it held instead of keeping it.
            self.span = None;
            step.messages.push((None, ScanResult::GateEmpty));
            return step;
        }
        if let (Some(first), Some(last)) = (bars.first(), bars.last()) {
            self.span = Some(match self.span {
                Some((a, b)) => (a.min(first.0), b.max(last.1)),
                None => (first.0, last.1),
            });
        }
        match motion {
            Motion::Scrolled(dy) => {
                let dy = i64::from(dy) * i64::from(UPSCALE);
                self.mark.offset += dy;
                self.moved = true;
                let (a, b) = self.span.unwrap_or((0, dims.1));
                let span = (i64::from(a * UPSCALE), i64::from(b * UPSCALE));
                step.messages.push((None, ScanResult::Scrolled { dy, to: self.mark, span }));
            }
            Motion::Lost => {
                // Positions from before are meaningless from here on: a
                // new epoch, and nothing is read off a transition frame.
                self.mark = ScrollMark { epoch: self.mark.epoch + 1, offset: 0 };
                self.moved = true;
                step.messages.push((None, ScanResult::TrackingLost { to: self.mark }));
                return step;
            }
            Motion::Still => {}
        }
        if bars.is_empty() {
            // A still frame without a bar is the close signal, at capture
            // cadence rather than tesseract's.
            if motion == Motion::Still {
                step.messages.push((None, ScanResult::NoBands));
            }
            return step;
        }
        let pass = resolver.resolve(&frame, &bars);
        let owed = |r: &Priced| pass.tickets.iter().any(|&(y0, _)| y0 * UPSCALE == r.y_top);
        if !pass.unresolved && pass.tickets.is_empty() && !pass.rows.is_empty() {
            let scan = Scan { rows: pass.rows, stale: pass.stale, at: Some(self.mark), partial: false };
            step.messages.push((pass.league, ScanResult::Rows(scan)));
            return step;
        }
        // Confirmed template hits show now, on this frame's positions; the
        // rest of the panel waits for tesseract.
        let known: Vec<Priced> = pass.rows.iter().filter(|r| !owed(r)).cloned().collect();
        if !known.is_empty() {
            let scan = Scan { rows: known, stale: pass.stale, at: Some(self.mark), partial: true };
            step.messages.push((pass.league.clone(), ScanResult::Rows(scan)));
        }
        // Tesseract reads still frames only: mid-scroll rows can be cut by
        // the viewport's edges.
        let due = self.moved || self.last_job.is_none_or(|t| now.duration_since(t) >= HEAVY_EVERY);
        if motion == Motion::Still && due {
            let with_whole = !std::mem::take(&mut self.moved);
            self.last_job = Some(now);
            step.job = Some(OcrJob { frame: Arc::new(frame), bars, with_whole, templates: pass, at: self.mark });
        }
        step
    }
}

/// The pipeline's two threads.
pub struct Workers {
    tracker: std::thread::JoinHandle<()>,
    reader: std::thread::JoinHandle<()>,
}

impl Workers {
    /// Waits for both threads; they end when the frames do.
    pub fn join(self) {
        let _ = self.tracker.join();
        let _ = self.reader.join();
    }
}

/// The newest job not yet started, and whether the frames have ended.
type Handoff = Arc<(Mutex<(Option<OcrJob>, bool)>, Condvar)>;

/// Starts the pipeline on `frames`. `panel_open` follows the bar gate (the
/// capture picks its cadence from it); messages go to `out` in order.
pub fn spawn<F, R, D>(
    frames: F,
    mut resolver: R,
    mut reader: D,
    out: Sender<Message>,
    panel_open: Arc<AtomicBool>,
) -> std::io::Result<Workers>
where
    F: IntoIterator<Item = Frame> + Send + 'static,
    R: Resolve + Send + 'static,
    D: ReadRows + Send + 'static,
{
    let trace = std::env::var("KHALONI_DEBUG").is_ok();
    let handoff: Handoff = Arc::default();
    let read_out = out.clone();
    let slot = handoff.clone();
    let reader = std::thread::Builder::new().name("reward-ocr".into()).spawn(move || loop {
        let job = {
            let (lock, ready) = &*slot;
            let mut state = lock.lock().unwrap_or_else(|e| e.into_inner());
            loop {
                if let Some(job) = state.0.take() {
                    break job;
                }
                if state.1 {
                    return;
                }
                state = ready.wait(state).unwrap_or_else(|e| e.into_inner());
            }
        };
        if let Some(read) = reader.read(&job) {
            if read_out.send(finish(&job, read)).is_err() {
                return;
            }
        }
    })?;
    let tracker = std::thread::Builder::new().name("reward-track".into()).spawn(move || {
        let t0 = Instant::now();
        let mut tracker = Tracker::new();
        let mut was_open = false;
        for frame in frames {
            let t = Instant::now();
            let step = tracker.step(frame, t, &mut resolver);
            panel_open.store(step.open, Ordering::Relaxed);
            if trace && (step.open != was_open || step.motion != Motion::Still) {
                eprintln!(
                    "TRACE {:>8.2}s bars={} gate_open={} motion={:?} in {:?}",
                    t0.elapsed().as_secs_f32(),
                    step.bars,
                    step.open,
                    step.motion,
                    t.elapsed()
                );
            }
            was_open = step.open;
            for m in step.messages {
                if out.send(m).is_err() {
                    break;
                }
            }
            if let Some(job) = step.job {
                let (lock, ready) = &*handoff;
                lock.lock().unwrap_or_else(|e| e.into_inner()).0 = Some(job);
                ready.notify_one();
            }
        }
        let (lock, ready) = &*handoff;
        lock.lock().unwrap_or_else(|e| e.into_inner()).1 = true;
        ready.notify_one();
    })?;
    Ok(Workers { tracker, reader })
}
