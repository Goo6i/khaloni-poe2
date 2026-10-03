//! The reward rows of a long, scrolling list, driven frame by frame
//! through the real pipeline: motion tracking, the bar gate, template
//! resolution and the stabilizer, with a stand-in for tesseract.
//!
//! The frames are built from the two real rows of the live 4K fixture,
//! the way the game draws a long list: a static header (the book title),
//! a scrolling viewport of reward rows, a static footer (the rest of the
//! page) and a static book border right of the bars. Every row differs
//! from its neighbours: rows alternate between the two real rows, and each
//! pair has its text and icons moved sideways by a different amount, as
//! reward names of different lengths would be. (Two identical rows two
//! apart would make a scroll of two rows pixel-identical to no scroll at
//! all, which no tracker could tell apart.)

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::{mpsc, Arc, OnceLock};
use std::time::{Duration, Instant};

use image::{imageops, GrayImage};
use khaloni_poe2::ocr::{self, Motion, RowSignature, UiScale, UPSCALE};
use khaloni_poe2::pricing::{Denom, Priced, Tier};
use khaloni_poe2::reward_pipeline::{
    self as rp, Frame, OcrJob, ReadOut, ReadRows, Resolve, TemplatePass, Tracker,
};
use khaloni_poe2::stabilize::{ScanResult, ScrollMark, Stabilizer};

/// The region the detector reports on the live fixture (tests/bars.rs).
const LIVE_REGION: (u32, u32, u32, u32) = (96, 144, 1088, 1244);
/// Rows in the synthesized list: long enough to scroll well past a
/// viewport and a half.
const ROWS: u32 = 20;
/// Static page rows below the viewport.
const FOOTER: u32 = 300;
/// Columns of a bar whose content differs between rows: right of the
/// first four icons, left of the bar's end.
const TEXT_X0: u32 = 326;
const TEXT_X1: u32 = 980;
/// The bars end at x ~992; the book border right of them does not scroll.
const LIST_X1: u32 = 995;
/// Capture cadence while the panel is open.
const FRAME: Duration = Duration::from_millis(16);

struct Panel {
    base: GrayImage,
    list: GrayImage,
    header: u32,
    pitch: u32,
    /// Bar heights of the two real rows (77 and 78 px).
    bar_h: [u32; 2],
    view_h: u32,
    /// Content key of each row's bar, for the stand-in reader.
    keys: HashMap<u64, usize>,
}

fn key_of(img: &GrayImage, y0: u32, y1: u32) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for y in y0..y1 {
        for x in TEXT_X0..TEXT_X1 {
            h ^= u64::from(img.get_pixel(x, y).0[0]);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

fn panel() -> &'static Panel {
    static PANEL: OnceLock<Panel> = OnceLock::new();
    PANEL.get_or_init(|| {
        let full = image::open(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/reward-live-1.png"))
            .expect("reward-live-1.png")
            .to_luma8();
        let (x, y, w, h) = LIVE_REGION;
        let base = imageops::crop_imm(&full, x, y, w, h).to_image();
        let bars = ocr::reward_bars_at(&base, &ocr::row_profile(&base), UiScale::REFERENCE);
        assert_eq!(bars.len(), 2, "the live book has two reward bars: {bars:?}");
        let header = bars[0].0;
        let pitch = bars[1].0 - bars[0].0;
        let bar_h = [bars[0].1 - bars[0].0, bars[1].1 - bars[1].0];
        let mut list = GrayImage::new(w, ROWS * pitch);
        let span = TEXT_X1 - TEXT_X0;
        let mut keys = HashMap::new();
        for i in 0..ROWS {
            let src = bars[(i % 2) as usize].0;
            let mut strip = imageops::crop_imm(&base, 0, src, w, pitch).to_image();
            let shift = (i / 2) * 61 % span;
            let orig = strip.clone();
            for yy in 0..pitch {
                for xx in 0..span {
                    let from = TEXT_X0 + (xx + span - shift) % span;
                    strip.put_pixel(TEXT_X0 + xx, yy, *orig.get_pixel(from, yy));
                }
            }
            imageops::replace(&mut list, &strip, 0, i64::from(i * pitch));
            let k = key_of(&list, i * pitch, i * pitch + bar_h[(i % 2) as usize]);
            assert!(keys.insert(k, i as usize).is_none(), "row {i} duplicates another row");
        }
        let view_h = h - header - FOOTER;
        Panel { base, list, header, pitch, bar_h, view_h, keys }
    })
}

impl Panel {
    fn bar_h(&self, i: usize) -> u32 {
        self.bar_h[i % 2]
    }

    fn max_scroll(&self) -> u32 {
        ROWS * self.pitch - self.view_h
    }

    /// The panel with the list scrolled `s` pixels down.
    fn frame(&self, s: u32) -> GrayImage {
        assert!(s <= self.max_scroll());
        let mut f = self.base.clone();
        let win = imageops::crop_imm(&self.list, 0, s, LIST_X1, self.view_h).to_image();
        imageops::replace(&mut f, &win, 0, i64::from(self.header));
        f
    }

    /// The list rows the viewport spans, in region pixels.
    fn span(&self) -> (u32, u32) {
        (self.header, self.header + self.view_h)
    }

    /// Where row `i`'s bar centre is at scroll `s`, in region pixels, when
    /// that centre is inside the viewport.
    fn centre(&self, i: usize, s: u32) -> Option<i64> {
        let c = i64::from(self.header) + i as i64 * i64::from(self.pitch) - i64::from(s) + i64::from(self.bar_h(i) / 2);
        let (a, b) = self.span();
        (c >= i64::from(a) && c < i64::from(b)).then_some(c)
    }

    /// Rows whose whole bar is inside the viewport at scroll `s`.
    fn whole_rows(&self, s: u32) -> Vec<usize> {
        (0..ROWS as usize)
            .filter(|&i| {
                let top = i64::from(self.header) + i as i64 * i64::from(self.pitch) - i64::from(s);
                top >= i64::from(self.header) && top + i64::from(self.bar_h(i)) <= i64::from(self.header + self.view_h)
            })
            .collect()
    }

    /// Which row a detected bar shows: only a whole bar can be read.
    fn identify(&self, gray: &GrayImage, (y0, y1): (u32, u32)) -> Option<usize> {
        self.keys.get(&key_of(gray, y0, y1)).copied()
    }

    fn sig(&self, s: u32) -> RowSignature {
        RowSignature::of(&self.frame(s))
    }
}

fn priced(i: usize, (y0, y1): (u32, u32)) -> Priced {
    Priced {
        y_top: y0 * UPSCALE,
        height: (y1 - y0) * UPSCALE,
        label: format!("{i} ex"),
        amount: i.to_string(),
        denom: Denom::Exalted,
        tier: Tier::Decent,
        item_key: format!("row{i}"),
        count: 1,
        value_ex: i as f64 + 1.0,
        value_chaos: 0.0,
        count_explicit: false,
        locks_in_one: true,
    }
}

/// Template store stand-in: knows the rows in `learned`.
struct Templates {
    learned: Vec<usize>,
}

impl Resolve for Templates {
    fn resolve(&mut self, frame: &Frame, bars: &[(u32, u32)]) -> TemplatePass {
        let p = panel();
        let mut pass = TemplatePass::default();
        for &bar in bars {
            match p.identify(&frame.gray, bar).filter(|i| self.learned.contains(i)) {
                Some(i) => pass.rows.push(priced(i, bar)),
                None => pass.unresolved = true,
            }
        }
        pass
    }
}

/// Tesseract stand-in: reads every whole bar exactly.
struct Reader;

impl ReadRows for Reader {
    fn read(&mut self, job: &OcrJob) -> Option<ReadOut> {
        let p = panel();
        let mut rows: Vec<Priced> = job.templates.rows.clone();
        for &bar in &job.bars {
            if rows.iter().any(|r| r.y_top == bar.0 * UPSCALE) {
                continue;
            }
            if let Some(i) = p.identify(&job.frame.gray, bar) {
                rows.push(priced(i, bar));
            }
        }
        rows.sort_by_key(|r| r.y_top);
        Some(ReadOut { rows, league: None, stale: false })
    }
}

/// The pipeline and the stabilizer on one clock, with a reader that takes
/// `latency` frames per read and only ever starts on the newest job, as
/// the reader thread does.
struct Replay {
    tracker: Tracker,
    stab: Stabilizer,
    templates: Templates,
    reader: Reader,
    latency: usize,
    queued: Option<OcrJob>,
    reading: Option<(OcrJob, usize)>,
    frame_no: usize,
    t0: Instant,
    reads: usize,
}

impl Replay {
    fn new(learned: Vec<usize>, latency: usize) -> Replay {
        Replay {
            tracker: Tracker::new(),
            stab: Stabilizer::new(),
            templates: Templates { learned },
            reader: Reader,
            latency,
            queued: None,
            reading: None,
            frame_no: 0,
            t0: Instant::now(),
            reads: 0,
        }
    }

    fn step(&mut self, s: u32) {
        let now = self.t0 + FRAME * self.frame_no as u32;
        let frame = Frame { gray: panel().frame(s), scale: UiScale::REFERENCE };
        let step = self.tracker.step(frame, now, &mut self.templates);
        for (_, msg) in step.messages {
            self.stab.apply(msg);
        }
        if let Some(job) = step.job {
            self.queued = Some(job);
        }
        if self.reading.is_none() {
            if let Some(job) = self.queued.take() {
                self.reading = Some((job, self.frame_no + self.latency));
            }
        }
        if self.reading.as_ref().is_some_and(|(_, done)| *done <= self.frame_no) {
            let (job, _) = self.reading.take().expect("checked");
            let out = self.reader.read(&job).expect("the stand-in always reads");
            self.reads += 1;
            self.stab.apply(rp::finish(&job, out).1);
        }
        self.frame_no += 1;
    }

    /// Displayed rows as (row index, centre in region px).
    fn shown(&self) -> Vec<(usize, i64)> {
        self.stab
            .rows()
            .iter()
            .map(|r| {
                let i: usize = r.item_key.trim_start_matches("row").parse().expect("row key");
                (i, i64::from(r.y_top + r.height / 2) / i64::from(UPSCALE))
            })
            .collect()
    }

    /// Every displayed price sits on the row it names, and that row is on
    /// screen. `TOL` is the stabilizer's jitter snap (22 preprocessed px)
    /// in region pixels, rounded up.
    fn assert_on_rows(&self, s: u32, when: &str) {
        const TOL: i64 = 8;
        let p = panel();
        for (i, y) in self.shown() {
            let Some(want) = p.centre(i, s) else {
                panic!("{when} (scroll {s}): row {i} is shown at y {y} but is off the list");
            };
            assert!(
                (y - want).abs() <= TOL,
                "{when} (scroll {s}): row {i} is shown at y {y}, its row is at {want}: {:?}",
                self.shown()
            );
        }
    }
}

/// Scroll positions from `from` to `to` in steps of `step`, ending on `to`.
fn path(from: u32, to: u32, step: u32) -> Vec<u32> {
    let mut out = Vec::new();
    let mut s = from;
    while s != to {
        s = if to > s { (s + step).min(to) } else { s.saturating_sub(step).max(to) };
        out.push(s);
    }
    out
}

#[test]
fn the_scroll_tracker_follows_the_real_panel_at_every_step() {
    let p = panel();
    let span = p.span();
    let max = p.max_scroll();
    for step in [1u32, 12, 40, 95, 190, 285, 380, 475, 570] {
        // A dozen start positions spread over the whole list, both ways.
        let starts: Vec<u32> = (0..12).map(|k| k * (max - step) / 11).collect();
        for s in starts {
            let (a, b) = (p.sig(s), p.sig(s + step));
            let t = Instant::now();
            let down = ocr::track_motion(&a, &b, span, UiScale::REFERENCE);
            let took = t.elapsed();
            assert_eq!(down, Motion::Scrolled(-(step as i32)), "step {step} down from {s} ({took:?})");
            assert_eq!(
                ocr::track_motion(&b, &a, span, UiScale::REFERENCE),
                Motion::Scrolled(step as i32),
                "step {step} up from {}",
                s + step
            );
        }
    }
    // No movement is Still.
    let a = p.sig(300);
    assert_eq!(ocr::track_motion(&a, &a, span, UiScale::REFERENCE), Motion::Still);
    // The same rows in another order are not a scroll of these ones.
    let mut other = p.frame(0);
    let (x, y) = (0u32, p.header);
    for k in 0..8u32 {
        let src = (7 - k) * p.pitch + 5 * p.pitch;
        let strip = imageops::crop_imm(&p.list, 0, src, LIST_X1, p.pitch).to_image();
        imageops::replace(&mut other, &strip, i64::from(x), i64::from(y + k * p.pitch));
    }
    assert_eq!(
        ocr::track_motion(&p.sig(0), &RowSignature::of(&other), span, UiScale::REFERENCE),
        Motion::Lost,
        "unrelated rows must not be taken for a scroll"
    );
}

#[test]
fn the_scroll_tracker_follows_the_panel_at_smaller_client_sizes() {
    // The same panel as a 1440p and a 1080p client draw it. The frames are
    // cut to 1242 rows so both sizes scale by an exact ratio, and every
    // step is a whole number of pixels at both.
    let p = panel();
    for height in [1440u32, 1080] {
        let scale = UiScale::from_frame_height(height);
        let small = |s: u32| -> GrayImage {
            let f = p.frame(s);
            let f = imageops::crop_imm(&f, 0, 0, f.width(), 1242).to_image();
            let (w, h) = (f.width() * height / 2160, 1242 * height / 2160);
            imageops::resize(&f, w, h, imageops::FilterType::Triangle)
        };
        let first = small(0);
        let bars = ocr::reward_bars_at(&first, &ocr::row_profile(&first), scale);
        assert!(bars.len() >= 7, "{height}p: the viewport's bars: {bars:?}");
        let span = (bars[0].0, bars[bars.len() - 1].1);
        for s in [0, 300] {
            let from = RowSignature::of(&small(s));
            for step in [12u32, 42, 96, 192, 288, 570] {
                let got = ocr::track_motion(&from, &RowSignature::of(&small(s + step)), span, scale);
                let want = -((step * height / 2160) as i32);
                assert_eq!(got, Motion::Scrolled(want), "{height}p: step {step} from {s}");
            }
        }
    }
}

#[test]
fn a_scroll_of_whole_rows_is_not_mistaken_for_no_movement() {
    let p = panel();
    let span = p.span();
    for rows in 1..=4u32 {
        let step = rows * p.pitch;
        for s in [0, p.pitch / 2, 2 * p.pitch + 7, p.max_scroll() - step] {
            let got = ocr::track_motion(&p.sig(s), &p.sig(s + step), span, UiScale::REFERENCE);
            assert_eq!(got, Motion::Scrolled(-(step as i32)), "{rows} whole rows down from {s}");
        }
    }
    // Through the pipeline too: a one-row scroll is reported as a scroll.
    let mut r = Replay::new(vec![], 0);
    for _ in 0..3 {
        r.step(0);
    }
    let step = r.tracker.step(
        Frame { gray: p.frame(p.pitch), scale: UiScale::REFERENCE },
        r.t0 + FRAME * 3,
        &mut r.templates,
    );
    assert!(
        step.messages.iter().any(|(_, m)| matches!(m, ScanResult::Scrolled { dy, .. } if *dy == -i64::from(p.pitch * UPSCALE))),
        "a whole-row scroll must reach the stabilizer as a scroll"
    );
}

#[test]
fn an_ocr_result_read_before_a_scroll_lands_on_its_row_after_it() {
    let p = panel();
    // Tesseract takes ~260 ms: 16 frames. Nothing is learned, so every
    // price comes from a read.
    let mut r = Replay::new(vec![], 16);
    for _ in 0..3 {
        r.step(0);
    }
    assert!(r.reading.is_some(), "a read of the still panel is under way");
    assert!(r.shown().is_empty());
    // While it runs, the list scrolls by two rows and a notch.
    let mut s = 0;
    for next in path(0, 2 * p.pitch + 40, 40) {
        s = next;
        r.step(s);
    }
    // Hold still until that first read lands.
    while r.reads == 0 {
        r.step(s);
    }
    assert!(!r.shown().is_empty(), "the read made before the scroll shows its rows");
    r.assert_on_rows(s, "the read made before the scroll");
}

#[test]
fn motion_is_tracked_while_ocr_is_busy() {
    let p = panel();
    struct Blocked {
        release: mpsc::Receiver<()>,
        started: mpsc::Sender<()>,
    }
    impl ReadRows for Blocked {
        fn read(&mut self, _job: &OcrJob) -> Option<ReadOut> {
            let _ = self.started.send(());
            let _ = self.release.recv();
            None
        }
    }
    let (release_tx, release_rx) = mpsc::channel();
    let (started_tx, started_rx) = mpsc::channel();
    let (frame_tx, frame_rx) = mpsc::channel::<Frame>();
    let (out_tx, out_rx) = mpsc::channel();
    let workers = rp::spawn(
        frame_rx,
        Templates { learned: vec![] },
        Blocked { release: release_rx, started: started_tx },
        out_tx,
        Arc::new(AtomicBool::new(false)),
    )
    .expect("spawn");
    for _ in 0..3 {
        frame_tx.send(Frame { gray: p.frame(0), scale: UiScale::REFERENCE }).unwrap();
    }
    started_rx.recv_timeout(Duration::from_secs(20)).expect("a read starts on the still panel");
    // The reader is now stuck; the list scrolls six notches.
    let mut moved = 0i64;
    for s in path(0, 240, 40) {
        frame_tx.send(Frame { gray: p.frame(s), scale: UiScale::REFERENCE }).unwrap();
    }
    while moved != -240 * i64::from(UPSCALE) {
        match out_rx.recv_timeout(Duration::from_secs(20)) {
            Ok((_, ScanResult::Scrolled { dy, .. })) => moved += dy,
            Ok(_) => {}
            Err(_) => panic!("scrolls must be reported while tesseract is busy (seen {moved})"),
        }
    }
    release_tx.send(()).unwrap();
    drop(frame_tx);
    drop(release_tx);
    workers.join();
}

/// The panel's list span in preprocessed pixels, as a scroll carries it.
fn span_pre(p: &Panel) -> (i64, i64) {
    let (a, b) = p.span();
    (i64::from(a * UPSCALE), i64::from(b * UPSCALE))
}

fn scrolled(dy: i64, offset: i64, span: (i64, i64)) -> ScanResult {
    ScanResult::Scrolled { dy, to: ScrollMark { epoch: 0, offset }, span }
}

#[test]
fn a_row_scrolled_above_the_list_leaves_instead_of_pinning_to_the_top() {
    let p = panel();
    let span = span_pre(p);
    let mut s = Stabilizer::new();
    // Rows 0-2 of the list at rest.
    let bars: Vec<(u32, u32)> =
        (0..3).map(|i| (p.header + i * p.pitch, p.header + i * p.pitch + p.bar_h(i as usize))).collect();
    s.apply(ScanResult::rows(bars.iter().enumerate().map(|(i, &b)| priced(i, b)).collect(), false));
    assert_eq!(s.rows().len(), 3);
    // Two rows and a bit down: rows 0 and 1 have left over the header,
    // row 0 by less than its own height.
    let dy = -i64::from((2 * p.pitch + 20) * UPSCALE);
    s.apply(scrolled(dy, dy, span));
    let rows = s.rows();
    let keys: Vec<&str> = rows.iter().map(|r| r.item_key.as_str()).collect();
    assert_eq!(keys, ["row2"], "rows 0 and 1 are gone, not pinned to the top");
    assert_eq!(i64::from(rows[0].y_top), i64::from(bars[2].0 * UPSCALE) + dy, "row 2 moved with the list");
}

#[test]
fn a_row_scrolled_below_the_list_leaves_the_display() {
    let p = panel();
    let span = span_pre(p);
    let mut s = Stabilizer::new();
    // The last two whole rows of the viewport at rest.
    let whole = p.whole_rows(0);
    let last = *whole.last().expect("rows");
    let bars: Vec<(usize, (u32, u32))> = [last - 1, last]
        .into_iter()
        .map(|i| (i, (p.header + i as u32 * p.pitch, p.header + i as u32 * p.pitch + p.bar_h(i))))
        .collect();
    s.apply(ScanResult::rows(bars.iter().map(|&(i, b)| priced(i, b)).collect(), false));
    assert_eq!(s.rows().len(), 2);
    // Scrolling back up by a row and a notch pushes the last one below
    // the list's bottom edge.
    let dy = i64::from((p.pitch + 40) * UPSCALE);
    s.apply(scrolled(dy, dy, span));
    let keys: Vec<String> = s.rows().iter().map(|r| r.item_key.clone()).collect();
    assert_eq!(keys, [format!("row{}", last - 1)], "the row pushed below the list leaves");
    for r in s.rows() {
        assert!(i64::from(r.y_top + r.height / 2) < span.1, "{} hangs below the list", r.item_key);
    }
}

#[test]
fn a_scrolled_reward_list_keeps_every_price_on_its_row() {
    let p = panel();
    // Some rows are known to the template store, the rest wait for a read
    // that takes 16 frames.
    let mut r = Replay::new(vec![0, 3, 4, 9, 13, 17], 16);
    let pitch = p.pitch;
    let mut script: Vec<u32> = vec![0; 30];
    let at = |script: &mut Vec<u32>, to: u32, step: u32, rest: usize| {
        let from = *script.last().unwrap();
        script.extend(path(from, to, step));
        script.extend(std::iter::repeat_n(to, rest));
    };
    at(&mut script, 240, 12, 20); // a smooth drag
    at(&mut script, 360, 40, 4); // wheel notches
    at(&mut script, 480, 40, 25);
    at(&mut script, 480 + pitch, pitch, 25); // exactly one row per frame
    at(&mut script, 480 + 4 * pitch, pitch, 25);
    at(&mut script, 480 + 4 * pitch - 3 * pitch, 3 * pitch, 25); // an OCR scan's worth at once, back up
    at(&mut script, p.max_scroll(), 40, 25);
    at(&mut script, 100, 285, 25);
    at(&mut script, 0, 12, 30);
    for (n, &s) in script.iter().enumerate() {
        r.step(s);
        r.assert_on_rows(s, &format!("frame {n}"));
    }
    // At rest, every whole row carries its price.
    let shown: Vec<usize> = r.shown().iter().map(|&(i, _)| i).collect();
    for i in p.whole_rows(0) {
        assert!(shown.contains(&i), "row {i} is whole on screen but unpriced: {shown:?}");
    }
}

#[test]
fn revealed_rows_are_priced_within_a_few_frames() {
    // The bound: a row the template store knows is priced on the frame its
    // bar is first whole on screen; any other row by the first frame after
    // the scroll comes to rest, plus the read's own time. Here a read takes
    // no frames, and each wheel notch moves the list over 3 frames, so a
    // revealed row is priced within 3 frames of its bar becoming whole.
    const BOUND: usize = 3;
    let p = panel();
    let learned = vec![8, 11];
    let mut r = Replay::new(learned.clone(), 0);
    let mut script: Vec<u32> = vec![0; 10];
    for notch in 1..=6u32 {
        script.extend(path((notch - 1) * 120, notch * 120, 40));
        script.extend([notch * 120; 4]);
    }
    let mut whole_since: HashMap<usize, usize> = HashMap::new();
    let mut priced_at: HashMap<usize, usize> = HashMap::new();
    let mut before: Vec<usize> = Vec::new();
    for (n, &s) in script.iter().enumerate() {
        r.step(s);
        r.assert_on_rows(s, &format!("frame {n}"));
        let whole = p.whole_rows(s);
        let shown: Vec<usize> = r.shown().iter().map(|&(i, _)| i).collect();
        for &i in &whole {
            whole_since.entry(i).or_insert(n);
            if shown.contains(&i) {
                priced_at.entry(i).or_insert(n);
            }
        }
        // A scroll never hides the prices of rows that stay on screen.
        for i in before.iter().filter(|i| whole.contains(i)) {
            assert!(shown.contains(i), "frame {n}: row {i} lost its price mid-scroll: {shown:?}");
        }
        before = shown;
    }
    let first_view = p.whole_rows(0);
    for (&i, &since) in &whole_since {
        if first_view.contains(&i) {
            continue;
        }
        let at = *priced_at.get(&i).unwrap_or_else(|| panic!("revealed row {i} was never priced"));
        let bound = if learned.contains(&i) { 0 } else { BOUND };
        assert!(at - since <= bound, "row {i} whole at frame {since}, priced at {at}: over {bound} frames");
    }
}
