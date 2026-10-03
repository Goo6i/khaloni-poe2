use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use fontdue::{Font, FontSettings, Metrics};
use image::RgbaImage;
use tiny_skia::{Color, Paint, PathBuilder, Pixmap, Rect as SkRect, Stroke, Transform};

use crate::hover::Popup;
use crate::pricing::{Denom, Tier};

#[derive(Clone, PartialEq)]
pub struct Placed {
    pub x: i32,
    pub y: i32,
    pub amount: String,
    pub denom: Denom,
    pub tier: Tier,
    /// Highest-value row of a pick-one panel: drawn with a gold border
    /// and a small crown mark so the best choice reads at a glance.
    pub best: bool,
}

/// A rumour rating badge placed in surface-local pixels: `x` is the badge's
/// left edge (hung off the tooltip panel's right side), `y` the vertical
/// center of the rumour's text line, `rating` the sheet rating ("S+", "A", ...).
#[derive(Clone, PartialEq)]
pub struct RumourBadge {
    pub x: i32,
    pub y: i32,
    pub rating: String,
}

// Which embedded Fontin face a glyph came from; part of the cache key so the
// SmallCaps amount glyphs and the Regular annotation glyphs never collide.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum FontKind {
    Amount,
    Annotation,
}

// Keyed by (face, char, DEVICE font size in integer tenths of a pixel), so
// glyphs rasterized for one output scale are never drawn at another.
type GlyphKey = (FontKind, char, u32);
type GlyphCache = RefCell<HashMap<GlyphKey, (Metrics, Vec<u8>)>>;

// Bundles a text draw's face/size/color so `draw_text` stays under clippy's
// too-many-arguments threshold.
#[derive(Clone, Copy)]
struct TextStyle {
    kind: FontKind,
    px: f32,
    color: Color,
}

pub struct Renderer {
    amount_font: Font,
    annotation_font: Font,
    glyph_cache: GlyphCache,
    /// Device pixels per logical pixel; see [`Renderer::set_scale`].
    scale: Cell<f32>,
    /// The icon artwork at its source resolution, and the copies resized to
    /// `ICON_SIZE * scale` device pixels that are actually composited.
    icon_divine: RgbaImage,
    icon_exalted: RgbaImage,
    icon_chaos: RgbaImage,
    icons_scaled: RefCell<HashMap<(u8, u32), RgbaImage>>,
}

const FONTIN_REGULAR: &[u8] = include_bytes!("../assets/fonts/Fontin-Regular.ttf");
const FONTIN_SMALLCAPS: &[u8] = include_bytes!("../assets/fonts/Fontin-SmallCaps.ttf");
const ICON_DIVINE_PNG: &[u8] = include_bytes!("../assets/icons/divine.png");
const ICON_EXALTED_PNG: &[u8] = include_bytes!("../assets/icons/exalted.png");
const ICON_CHAOS_PNG: &[u8] = include_bytes!("../assets/icons/chaos.png");

const AMOUNT_PX: f32 = 22.0;
const OLD_PX: f32 = 13.0;
const TOTAL_PX: f32 = 20.0;
const ICON_SIZE: u32 = 30;
const ICON_GAP: f32 = 4.0;
const PILL_PAD_X: f32 = 8.0;
const PILL_CORNER: f32 = 4.0;
const PILL_BORDER_WIDTH: f32 = 1.5;

// Hover price-check popup (Stage A: display-only, no interactivity).
/// Minimum popup width; the box grows to fit its widest text line (long
/// item names and waystone mod lines must never overflow the pill —
/// live finding from the first Windows testers).
const POPUP_MIN_WIDTH: f32 = 320.0;
const POPUP_TITLE_PX: f32 = 22.0;
const POPUP_LINE_PX: f32 = 18.0;
const POPUP_PAD: f32 = 12.0;
const POPUP_ROW_GAP: f32 = 6.0;

// Shared design system with the control panel (settings-mockup.html): warm
// near-black chrome, bronze hairlines, off-white text, grey/blue/gold value
// tiers. The overlay reads as one tool with the settings window.
const C_PANEL: (u8, u8, u8, u8) = (0x1C, 0x16, 0x0F, 238); // pill/panel fill (near-opaque over the game)
const C_INK: (u8, u8, u8) = (0xEA, 0xE0, 0xCB); // primary text
const C_INK2: (u8, u8, u8) = (0xB3, 0xA3, 0x82); // secondary / descriptions
const C_LINE: (u8, u8, u8) = (0x37, 0x2C, 0x1E); // subtle hairline
const C_BRONZE: (u8, u8, u8) = (0x6B, 0x56, 0x37); // default border
const C_GOLD: (u8, u8, u8) = (0xC9, 0xA2, 0x27); // jackpot / best
const C_BLUE: (u8, u8, u8) = (0x2E, 0x5A, 0x8A); // decent border
const C_BLUE_LT: (u8, u8, u8) = (0x7F, 0xA8, 0xD6); // decent text (readable on dark)
const C_JUNK_LT: (u8, u8, u8) = (0xB7, 0xAB, 0x97); // junk text
const C_RED: (u8, u8, u8) = (0x8B, 0x3A, 0x2E); // stale / danger
const C_UNIQUE: (u8, u8, u8) = (0xAF, 0x60, 0x25); // unique item name, the game's own orange
// Suffix badges: a warm amber against the prefixes' C_BLUE_LT, so the two
// affix families split cool/warm at a glance without inventing a hue
// outside the design system.
const C_SUFFIX: (u8, u8, u8) = (0xD9, 0x9A, 0x4A);
// Dark inset behind an editable value box (same well as the reference
// panel's search field).
const C_WELL: (u8, u8, u8) = (0x12, 0x0D, 0x08);
// Market moves: a soft green and a soft red that both read on the dark
// panel beside the gold headings.
const C_UP: (u8, u8, u8) = (0x7F, 0xB8, 0x6A);
const C_DOWN: (u8, u8, u8) = (0xD9, 0x7A, 0x5F);

// Evaluate item-card type scale: a large small-caps name, tooltip-sized
// property lines, and deliberately small gutter type so the mod text stays
// the thing the eye lands on.
const EVAL_NAME_PX: f32 = 24.0;
const EVAL_PROP_PX: f32 = 15.0;
const EVAL_COL_PX: f32 = 11.0;
const EVAL_BADGE_PX: f32 = 13.0;
const EVAL_SCORE_PX: f32 = 14.0;
const EVAL_BOX_PX: f32 = 14.0;
/// The remark after a searchable row's line ("counted in Total Life").
const EVAL_NOTE_PX: f32 = 13.0;

fn rgb(c: (u8, u8, u8)) -> Color {
    Color::from_rgba8(c.0, c.1, c.2, 0xFF)
}

/// Formats a value-box number: whole numbers show without a decimal point
/// (`155`), fractional ones keep their digits (`3.5`). Matches how the trade
/// search body is serialized, so the box shows exactly what gets searched.
fn fmt_num(v: f64) -> String {
    if v.fract() == 0.0 {
        format!("{}", v as i64)
    } else {
        // Trim to at most 2 decimals, then drop trailing zeros.
        let s = format!("{v:.2}");
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

/// Price text color per value tier (same grey/blue/gold ladder as settings,
/// lightened where needed to read on the dark pill).
fn amount_color(t: Tier) -> Color {
    match t {
        Tier::Junk | Tier::Unknown => rgb(C_JUNK_LT),
        Tier::Decent => rgb(C_BLUE_LT),
        Tier::Jackpot => rgb(C_GOLD),
    }
}

/// Pill border per tier: a bronze hairline for junk, the tier accent (blue /
/// gold) otherwise, mirroring the settings value-tier ladder's accent bars.
fn border_color(t: Tier) -> Color {
    match t {
        Tier::Junk | Tier::Unknown => rgb(C_LINE),
        Tier::Decent => rgb(C_BLUE),
        Tier::Jackpot => rgb(C_GOLD),
    }
}

fn stale_color() -> Color {
    rgb(C_RED)
}

fn best_border_color() -> Color {
    rgb(C_GOLD)
}

/// Neutral bronze hairline for container panels (popup, Evaluate, total),
/// matching the settings window's chrome rather than a value-tier accent.
fn panel_border() -> Color {
    rgb(C_BRONZE)
}

fn pill_fill_color() -> Color {
    Color::from_rgba8(C_PANEL.0, C_PANEL.1, C_PANEL.2, C_PANEL.3)
}

/// Rating-tier palette for rumour badges (from the rumour reference):
/// S=gold, A=green, B=blue, C=grey, D=orange, F=red. Keyed on the first
/// letter so "S+", "A+", "B+" collapse to their tier.
fn rating_color(rating: &str) -> Color {
    match rating.chars().next().map(|c| c.to_ascii_uppercase()) {
        Some('S') => Color::from_rgba8(0xC9, 0xA2, 0x27, 0xFF),
        Some('A') => Color::from_rgba8(0x3A, 0x8A, 0x3A, 0xFF),
        Some('B') => Color::from_rgba8(0x2E, 0x5A, 0x8A, 0xFF),
        Some('C') => Color::from_rgba8(0x6E, 0x65, 0x5A, 0xFF),
        Some('D') => Color::from_rgba8(0x9C, 0x4E, 0x12, 0xFF),
        Some('F') => Color::from_rgba8(0x8B, 0x3A, 0x2E, 0xFF),
        _ => Color::from_rgba8(0x6E, 0x65, 0x5A, 0xFF),
    }
}

const RATING_PX: f32 = 18.0;

impl Renderer {
    pub fn new() -> anyhow::Result<Renderer> {
        let amount_font = Font::from_bytes(FONTIN_SMALLCAPS, FontSettings::default())
            .map_err(|e| anyhow::anyhow!("Fontin-SmallCaps load: {e}"))?;
        let annotation_font = Font::from_bytes(FONTIN_REGULAR, FontSettings::default())
            .map_err(|e| anyhow::anyhow!("Fontin-Regular load: {e}"))?;
        Ok(Renderer {
            amount_font,
            annotation_font,
            glyph_cache: RefCell::new(HashMap::new()),
            scale: Cell::new(1.0),
            icon_divine: load_icon(ICON_DIVINE_PNG)?,
            icon_exalted: load_icon(ICON_EXALTED_PNG)?,
            icon_chaos: load_icon(ICON_CHAOS_PNG)?,
            icons_scaled: RefCell::new(HashMap::new()),
        })
    }

    /// Sets how many device pixels one logical pixel covers (1.5 on a 150%
    /// output). Every draw call keeps taking logical coordinates, the same
    /// ones layout and hit-testing use; the pixmap handed in is expected at
    /// device size. Glyphs are rasterized at `px * scale` and paths go
    /// through a scale transform, so text and hairlines come out sharp
    /// instead of being stretched by the compositor. Returns whether the
    /// scale changed (the caller then repaints).
    pub fn set_scale(&self, scale: f32) -> bool {
        let scale = if scale.is_finite() { scale.clamp(0.5, 4.0) } else { 1.0 };
        if (self.scale.get() - scale).abs() < 1e-3 {
            return false;
        }
        self.scale.set(scale);
        // Entries for the old scale would never be read again.
        self.glyph_cache.borrow_mut().clear();
        self.icons_scaled.borrow_mut().clear();
        true
    }

    pub fn scale(&self) -> f32 {
        self.scale.get()
    }

    /// Logical -> device transform for every path and rect.
    fn xf(&self) -> Transform {
        let s = self.scale.get();
        Transform::from_scale(s, s)
    }

    fn font_for(&self, kind: FontKind) -> &Font {
        match kind {
            FontKind::Amount => &self.amount_font,
            FontKind::Annotation => &self.annotation_font,
        }
    }

    fn icon_for(&self, denom: Denom) -> Option<&RgbaImage> {
        match denom {
            Denom::Divine => Some(&self.icon_divine),
            Denom::Exalted => Some(&self.icon_exalted),
            Denom::Chaos => Some(&self.icon_chaos),
            Denom::None => None,
        }
    }

    fn text_width(&self, kind: FontKind, text: &str, px: f32) -> f32 {
        let font = self.font_for(kind);
        text.chars().map(|c| font.metrics(c, px).advance_width).sum()
    }

    fn draw_text(&self, pm: &mut Pixmap, x: f32, y_baseline: f32, text: &str, style: &TextStyle) {
        let TextStyle { kind, px, color } = *style;
        let font = self.font_for(kind);
        // Logical in, device out: the glyphs are rasterized at the size
        // they will have on screen.
        let scale = self.scale.get();
        let px = px * scale;
        let y_baseline = y_baseline * scale;
        let mut pen = x * scale;
        let (cr, cg, cb) = (
            u32::from((color.red() * 255.0) as u16),
            u32::from((color.green() * 255.0) as u16),
            u32::from((color.blue() * 255.0) as u16),
        );
        let mut cache = self.glyph_cache.borrow_mut();
        for ch in text.chars() {
            let key: GlyphKey = (kind, ch, (px * 10.0).round() as u32);
            let (metrics, bitmap) = cache.entry(key).or_insert_with(|| font.rasterize(ch, px));
            let gx = pen as i32 + metrics.xmin;
            let gy = y_baseline as i32 - metrics.ymin - metrics.height as i32;
            let w = pm.width() as i32;
            let h = pm.height() as i32;
            let data = pm.data_mut();
            for (i, cov) in bitmap.iter().enumerate() {
                if *cov == 0 {
                    continue;
                }
                let px_x = gx + (i % metrics.width) as i32;
                let px_y = gy + (i / metrics.width) as i32;
                if px_x < 0 || px_y < 0 || px_x >= w || px_y >= h {
                    continue;
                }
                let idx = ((px_y * w + px_x) * 4) as usize;
                // The glyph bitmap is a coverage mask over a flat color: treat
                // it as a straight-alpha source and alpha-over composite it
                // onto the pixmap's existing premultiplied pixel, the same
                // math `composite_icon` uses. A coverage-max blend (as if the
                // background were always darker than the glyph) does not
                // work on the light parchment pill, since dark amount text
                // is *darker* than the fill it sits on.
                let sa = u32::from(*cov);
                let sr = cr * sa / 255;
                let sg = cg * sa / 255;
                let sb = cb * sa / 255;
                let inv = 255 - sa;
                data[idx] = (sr + u32::from(data[idx]) * inv / 255) as u8;
                data[idx + 1] = (sg + u32::from(data[idx + 1]) * inv / 255) as u8;
                data[idx + 2] = (sb + u32::from(data[idx + 2]) * inv / 255) as u8;
                data[idx + 3] = (sa + u32::from(data[idx + 3]) * inv / 255) as u8;
            }
            pen += metrics.advance_width;
        }
    }

    /// Alpha-over composites a straight-alpha RGBA icon onto the (natively
    /// premultiplied) pixmap buffer, `(x, y)` being the icon's top-left in
    /// logical pixels. The artwork is resized from its source resolution
    /// to the device size, not stretched from the 1x copy.
    fn composite_icon(&self, pm: &mut Pixmap, denom: Denom, x: i32, y: i32) {
        let Some(source) = self.icon_for(denom) else { return };
        let scale = self.scale.get();
        let side = ((ICON_SIZE as f32) * scale).round().max(1.0) as u32;
        let mut scaled = self.icons_scaled.borrow_mut();
        let icon = scaled.entry((denom as u8, side)).or_insert_with(|| {
            image::imageops::resize(source, side, side, image::imageops::FilterType::Lanczos3)
        });
        let x = (x as f32 * scale).round() as i32;
        let y = (y as f32 * scale).round() as i32;
        let w = pm.width() as i32;
        let h = pm.height() as i32;
        let data = pm.data_mut();
        for (ix, iy, px) in icon.enumerate_pixels() {
            let [r, g, b, a] = px.0;
            if a == 0 {
                continue;
            }
            let px_x = x + ix as i32;
            let px_y = y + iy as i32;
            if px_x < 0 || px_y < 0 || px_x >= w || px_y >= h {
                continue;
            }
            let idx = ((px_y * w + px_x) * 4) as usize;
            let sa = u32::from(a);
            let sr = u32::from(r) * sa / 255;
            let sg = u32::from(g) * sa / 255;
            let sb = u32::from(b) * sa / 255;
            let inv = 255 - sa;
            data[idx] = (sr + u32::from(data[idx]) * inv / 255) as u8;
            data[idx + 1] = (sg + u32::from(data[idx + 1]) * inv / 255) as u8;
            data[idx + 2] = (sb + u32::from(data[idx + 2]) * inv / 255) as u8;
            data[idx + 3] = (sa + u32::from(data[idx + 3]) * inv / 255) as u8;
        }
    }

    fn pill(&self, pm: &mut Pixmap, x: f32, y_top: f32, w: f32, h: f32, border: Color) {
        let Some(rect) = SkRect::from_xywh(x, y_top, w, h) else { return };
        let r = PILL_CORNER.min(w / 2.0).min(h / 2.0);
        let mut pb = PathBuilder::new();
        pb.move_to(rect.left() + r, rect.top());
        pb.line_to(rect.right() - r, rect.top());
        pb.quad_to(rect.right(), rect.top(), rect.right(), rect.top() + r);
        pb.line_to(rect.right(), rect.bottom() - r);
        pb.quad_to(rect.right(), rect.bottom(), rect.right() - r, rect.bottom());
        pb.line_to(rect.left() + r, rect.bottom());
        pb.quad_to(rect.left(), rect.bottom(), rect.left(), rect.bottom() - r);
        pb.line_to(rect.left(), rect.top() + r);
        pb.quad_to(rect.left(), rect.top(), rect.left() + r, rect.top());
        pb.close();
        let Some(path) = pb.finish() else { return };

        let mut fill_paint = Paint::default();
        fill_paint.set_color(pill_fill_color());
        fill_paint.anti_alias = true;
        pm.fill_path(&path, &fill_paint, tiny_skia::FillRule::Winding, self.xf(), None);

        let mut border_paint = Paint::default();
        border_paint.set_color(border);
        border_paint.anti_alias = true;
        let stroke = Stroke { width: PILL_BORDER_WIDTH, ..Default::default() };
        pm.stroke_path(&path, &border_paint, &stroke, self.xf(), None);
    }

    pub fn draw_frame(&self, pm: &mut Pixmap, labels: &[Placed], total: &str, stale: bool) {
        pm.fill(Color::TRANSPARENT);
        for p in labels {
            let icon = self.icon_for(p.denom);
            let amount_w = self.text_width(FontKind::Amount, &p.amount, AMOUNT_PX);
            let icon_w = if icon.is_some() { ICON_GAP + ICON_SIZE as f32 } else { 0.0 };
            let old_w = if stale { ICON_GAP + self.text_width(FontKind::Annotation, "(old)", OLD_PX) } else { 0.0 };
            // The BEST tag is pill content like everything else it draws
            // after; leaving it out let the text overflow the gold border.
            let best_w =
                if p.best { ICON_GAP + self.text_width(FontKind::Annotation, "BEST", OLD_PX) } else { 0.0 };

            let pill_h = AMOUNT_PX + 10.0;
            let content_w = amount_w + icon_w + old_w + best_w;
            let pill_x = p.x as f32 - PILL_PAD_X;
            let pill_y = p.y as f32 - pill_h / 2.0;
            let border = if p.best { best_border_color() } else { border_color(p.tier) };
            self.pill(pm, pill_x, pill_y, content_w + PILL_PAD_X * 2.0, pill_h, border);

            let baseline_y = p.y as f32 + AMOUNT_PX * 0.35;
            let mut pen_x = p.x as f32;
            let amount_style = TextStyle { kind: FontKind::Amount, px: AMOUNT_PX, color: amount_color(p.tier) };
            self.draw_text(pm, pen_x, baseline_y, &p.amount, &amount_style);
            pen_x += amount_w;

            if icon.is_some() {
                pen_x += ICON_GAP;
                let icon_y = p.y - (ICON_SIZE / 2) as i32;
                self.composite_icon(pm, p.denom, pen_x.round() as i32, icon_y);
                pen_x += ICON_SIZE as f32;
            }

            if stale {
                pen_x += ICON_GAP;
                let old_style = TextStyle { kind: FontKind::Annotation, px: OLD_PX, color: stale_color() };
                self.draw_text(pm, pen_x, p.y as f32 + OLD_PX * 0.35, "(old)", &old_style);
            }
            if p.best {
                pen_x += ICON_GAP;
                let best_style = TextStyle {
                    kind: FontKind::Annotation,
                    px: OLD_PX,
                    color: best_border_color(),
                };
                self.draw_text(pm, pen_x, p.y as f32 + OLD_PX * 0.35, "BEST", &best_style);
            }
        }
        if !total.is_empty() {
            if let Some(first) = labels.iter().min_by_key(|p| p.y) {
                let old_suffix = if stale { " (old)" } else { "" };
                let text = format!("{total}{old_suffix}");
                let tw = self.text_width(FontKind::Annotation, &text, TOTAL_PX);
                let pill_h = TOTAL_PX + 10.0;
                let y = (first.y - 30).max(14);
                self.pill(
                    pm,
                    first.x as f32 - PILL_PAD_X,
                    y as f32 - pill_h / 2.0,
                    tw + PILL_PAD_X * 2.0,
                    pill_h,
                    panel_border(),
                );
                let color = if stale { stale_color() } else { rgb(C_INK) };
                let total_style = TextStyle { kind: FontKind::Annotation, px: TOTAL_PX, color };
                self.draw_text(pm, first.x as f32, y as f32 + TOTAL_PX * 0.35, &text, &total_style);
            }
        }
    }

    /// Draws rumour rating badges: a parchment pill (same language as the
    /// reward rows) with the rating text in its tier color, hung off the
    /// tooltip panel's right edge at each rumour line. Call after
    /// `draw_frame` so badges sit on the already-cleared frame.
    pub fn draw_rumours(&self, pm: &mut Pixmap, badges: &[RumourBadge]) {
        for b in badges {
            let color = rating_color(&b.rating);
            let tw = self.text_width(FontKind::Amount, &b.rating, RATING_PX);
            let pill_h = RATING_PX + 10.0;
            let pill_w = tw + PILL_PAD_X * 2.0;
            self.pill(pm, b.x as f32, b.y as f32 - pill_h / 2.0, pill_w, pill_h, color);
            let style = TextStyle { kind: FontKind::Amount, px: RATING_PX, color };
            self.draw_text(
                pm,
                b.x as f32 + PILL_PAD_X,
                b.y as f32 + RATING_PX * 0.35,
                &b.rating,
                &style,
            );
        }
    }

    /// Rendered pixel width of an Evaluate row label, for the panel layout's
    /// `measure` callback. Same face and size `draw_evaluate` draws rows in,
    /// so the mod column is sized against the exact glyphs that land in it.
    pub fn evaluate_label_width(&self, text: &str) -> i32 {
        self.text_width(FontKind::Annotation, text, POPUP_LINE_PX).ceil() as i32
    }

    /// One tooltip property line: the small-caps label up to and including
    /// its colon in the muted ink, the value after it in the primary ink —
    /// how the game writes "Item Level: 81".
    fn draw_prop_line(&self, pm: &mut Pixmap, x: f32, baseline: f32, text: &str, px: f32) {
        let (label, value) = match text.find(':') {
            Some(i) => text.split_at(i + 1),
            None => (text, ""),
        };
        let label_style = TextStyle { kind: FontKind::Amount, px, color: rgb(C_INK2) };
        self.draw_text(pm, x, baseline, label, &label_style);
        if !value.is_empty() {
            let value_style = TextStyle { kind: FontKind::Amount, px, color: rgb(C_INK) };
            let lw = self.text_width(FontKind::Amount, label, px);
            self.draw_text(pm, x + lw, baseline, value, &value_style);
        }
    }

    /// A hairline rule across the card's inner width, the way the game's
    /// tooltip separates the name block from the properties from the mods.
    fn eval_rule(&self, pm: &mut Pixmap, ax: f32, y: f32, panel_w: f32) {
        self.pill(pm, ax + 12.0, y, panel_w - 24.0, 1.0, rgb(C_LINE));
    }

    /// A checkbox: hollow outline always, filled square when on. Same
    /// treatment every checkbox in the overlay uses.
    fn eval_check(&self, pm: &mut Pixmap, r: &crate::config::Rect, ax: f32, ay: f32, on: bool) {
        let (cx, cy) = (ax + r.x as f32, ay + r.y as f32);
        let side = r.w as f32;
        self.pill(pm, cx, cy, side, side, if on { rgb(C_INK) } else { rgb(C_INK2) });
        if on {
            let mut inner = Paint::default();
            inner.set_color(rgb(C_INK));
            if let Some(rect) = SkRect::from_xywh(cx + 4.0, cy + 4.0, side - 8.0, side - 8.0) {
                pm.fill_rect(rect, &inner, self.xf(), None);
            }
        }
    }

    /// The item name's colour by rarity: gold for rare, the readable blue
    /// for magic (the dark border blue disappears into the panel fill),
    /// the game's own orange for unique, plain ink otherwise.
    fn rarity_color(rarity: &str) -> Color {
        if rarity.eq_ignore_ascii_case("rare") {
            rgb(C_GOLD)
        } else if rarity.eq_ignore_ascii_case("magic") {
            rgb(C_BLUE_LT)
        } else if rarity.eq_ignore_ascii_case("unique") {
            rgb(C_UNIQUE)
        } else {
            rgb(C_INK)
        }
    }

    /// A block heading under the card, in the market view's section gold.
    fn section(&self, pm: &mut Pixmap, x: f32, baseline: f32, text: &str) {
        let style = TextStyle { kind: FontKind::Amount, px: 15.0, color: rgb(C_GOLD) };
        self.draw_text(pm, x, baseline, text, &style);
    }

    /// A table's caption line: small-caps column names with a hairline
    /// under them, the way the market tables head their columns.
    /// `anchor` is the panel's, `at` the caption baseline in panel pixels.
    fn captions(&self, pm: &mut Pixmap, anchor: (f32, f32), at: (i32, &[i32]), names: &[&str], w: f32) {
        let (ax, ay) = anchor;
        let (baseline, cols) = at;
        let style = TextStyle { kind: FontKind::Amount, px: EVAL_COL_PX, color: rgb(C_INK2) };
        for (x, name) in cols.iter().zip(names) {
            self.draw_text(pm, ax + *x as f32, ay + baseline as f32, name, &style);
        }
        self.pill(pm, ax + 12.0, ay + baseline as f32 + 4.0, w - 24.0, 1.0, rgb(C_LINE));
    }

    /// A small button: gold border and ink when live, hairline and muted
    /// when switched off, its label centred.
    fn small_button(&self, pm: &mut Pixmap, ax: f32, ay: f32, rect: &crate::config::Rect, label: &str, on: bool) {
        let (bx, by) = (ax + rect.x as f32, ay + rect.y as f32);
        let (bw, bh) = (rect.w as f32, rect.h as f32);
        self.pill(pm, bx, by, bw, bh, if on { border_color(Tier::Jackpot) } else { rgb(C_LINE) });
        let style = TextStyle { kind: FontKind::Amount, px: 14.0, color: if on { rgb(C_INK) } else { rgb(C_INK2) } };
        let tw = self.text_width(FontKind::Amount, label, 14.0);
        self.draw_text(pm, bx + (bw - tw).max(0.0) / 2.0, by + bh - 7.0, label, &style);
    }

    /// Draws the Evaluate item card with what the search found under it,
    /// from the SAME geometry the click handler hit-tests against
    /// (`evaluate_ui::layout`), offset to `anchor` (surface-local
    /// top-left). Every position comes from `lay`; nothing is recomputed
    /// here, so pixels and click targets cannot drift apart. The hover
    /// card, when a listing row is hovered, is drawn beside the panel from
    /// the same layout.
    ///
    /// `editing` names the box being typed into as (index into `panel.rows`,
    /// field).
    pub fn draw_evaluate(
        &self,
        pm: &mut Pixmap,
        panel: &crate::evaluate_ui::Panel,
        lay: &crate::evaluate_ui::Layout,
        anchor: (i32, i32),
        editing: Option<(usize, crate::evaluate_ui::Field)>,
        edit_buf: &str,
    ) {
        use crate::evaluate_ui::{AffixKind, Field, SellerState};
        let f = panel;
        let (ax, ay) = (anchor.0 as f32, anchor.1 as f32);
        let ink = rgb(C_INK);
        let dim = rgb(C_INK2);
        let w = lay.size.0 as f32;
        self.pill(pm, ax, ay, w, lay.size.1 as f32, panel_border());

        // Close X.
        let x_style = TextStyle { kind: FontKind::Amount, px: 18.0, color: ink };
        self.draw_text(pm, ax + lay.close.x as f32 + 4.0, ay + lay.close.y as f32 + 15.0, "x", &x_style);

        // Item name: centered like the game's tooltip header, coloured by
        // rarity, but falling back to the layout's left edge when centering
        // would run the name under the close X.
        let name_color = Self::rarity_color(&panel.header.rarity);
        let name_style = TextStyle { kind: FontKind::Amount, px: EVAL_NAME_PX, color: name_color };
        let nw = self.text_width(FontKind::Amount, &panel.header.name, EVAL_NAME_PX);
        let left = ax + lay.name_pos.0 as f32;
        let centered = ax + (w - nw) / 2.0;
        let limit = ax + lay.close.x as f32 - 8.0 - nw;
        let nx = if centered >= left && centered <= limit { centered } else { left };
        self.draw_text(pm, nx, ay + lay.name_pos.1 as f32, &panel.header.name, &name_style);

        // Property block: rarity, then whatever level lines the layout asked
        // for, all sharing the rarity line's left edge.
        let prop_x = ax + lay.rarity_pos.0 as f32;
        let mut last_prop_y = lay.rarity_pos.1;
        self.draw_prop_line(
            pm,
            prop_x,
            ay + lay.rarity_pos.1 as f32,
            &format!("Rarity: {}", panel.header.rarity),
            EVAL_PROP_PX,
        );
        for (y, text) in &lay.level_pos {
            self.draw_prop_line(pm, prop_x, ay + *y as f32, text, EVAL_PROP_PX);
            last_prop_y = *y;
        }
        // A stack's worth, as the caller worded it. The row face, not the
        // small caps: "37 x 3 ex" must not read as "37 X 3 EX".
        if let (Some(pos), Some(text)) = (&lay.stack_pos, &f.stack_value) {
            let style = TextStyle { kind: FontKind::Annotation, px: EVAL_PROP_PX, color: ink };
            self.draw_text(pm, prop_x, ay + pos.1 as f32, text, &style);
            last_prop_y = pos.1;
        }

        // Category constraint: unchecked, the query falls back to the
        // item's exact base, or to no base at all when it carries none.
        if let (Some(check), Some(base)) = (&lay.base_check, &panel.header.base) {
            self.eval_check(pm, check, ax, ay, base.enabled);
            let style = TextStyle {
                kind: FontKind::Annotation,
                px: POPUP_LINE_PX,
                color: if base.enabled { ink } else { dim },
            };
            self.draw_text(
                pm,
                ax + lay.base_label_pos.0 as f32,
                ay + lay.base_label_pos.1 as f32,
                &base.label,
                &style,
            );
            last_prop_y = lay.base_label_pos.1;
        }

        // Column headings, ruled above and below like a table header so the
        // gutters read as columns rather than as loose text.
        let head_y = lay.tiering_head_pos.1;
        let head_style = TextStyle { kind: FontKind::Amount, px: EVAL_COL_PX, color: dim };
        self.eval_rule(pm, ax, ay + ((last_prop_y + head_y) as f32 / 2.0 - 6.0).round(), w);
        self.draw_text(pm, ax + lay.tiering_head_pos.0 as f32, ay + head_y as f32, "TIERING", &head_style);
        // The scoring column runs from its gutter's left edge to the value
        // boxes; both the heading and the numbers centre in that span, so a
        // long mod line never reads as if it ran into its own score.
        let score_span = |g: &crate::evaluate_ui::RowGeom| (g.score_pos.0 as f32, g.min_box.x as f32 - 8.0);
        let (head_l, head_r) = lay
            .rows
            .first()
            .map(score_span)
            .unwrap_or((lay.scoring_head_pos.0 as f32, lay.scoring_head_pos.0 as f32 + 48.0));
        let hw = self.text_width(FontKind::Amount, "SCORING", EVAL_COL_PX);
        self.draw_text(
            pm,
            ax + (head_l + head_r - hw) / 2.0,
            ay + lay.scoring_head_pos.1 as f32,
            "SCORING",
            &head_style,
        );
        self.eval_rule(pm, ax, ay + head_y as f32 + 5.0, w);

        // Block separators (implicit vs explicit), from the same layout
        // the hitboxes use so the line can never sit on a row.
        for &dy in &lay.dividers {
            self.eval_rule(pm, ax, ay + dy as f32, lay.size.0 as f32);
        }
        for (g, &i) in lay.rows.iter().zip(&lay.visible_rows) {
            let Some(row) = panel.rows.get(i) else { continue };
            // Rows with no filter behind them (derived stats, unmatched
            // mods) are display-only: no checkbox, no value boxes, nothing
            // that implies they go to the search.
            let filterable = row.target.is_some();
            if filterable {
                self.eval_check(pm, &g.check, ax, ay, row.enabled);
            }

            if let Some(badge) = row.badge {
                let color = match badge.kind {
                    AffixKind::Prefix => rgb(C_BLUE_LT),
                    AffixKind::Suffix => rgb(C_SUFFIX),
                    AffixKind::Other => dim,
                };
                // "P9" / "S1", from the model's own formatter so the drawn
                // badge and the layout's column width agree.
                let text = crate::evaluate_ui::badge_text(&badge);
                let style = TextStyle { kind: FontKind::Amount, px: EVAL_BADGE_PX, color };
                self.draw_text(pm, ax + g.badge_pos.0 as f32, ay + g.badge_pos.1 as f32, &text, &style);
            }

            let (lx, ly) = (ax + g.label_pos.0 as f32, ay + g.label_pos.1 as f32);
            let off = filterable && !row.enabled;
            let mut label_style =
                TextStyle { kind: FontKind::Annotation, px: POPUP_LINE_PX, color: if off { dim } else { ink } };
            let property_colon = row.label.find(':').filter(|_| !filterable && row.note.is_none());
            match (property_colon, &row.note) {
                // A line with a note beside it that can be searched (one a
                // total already counts): the line reads like any row, and
                // the note follows it small and dim, a remark on the row
                // rather than part of it.
                (None, Some(note)) if filterable => {
                    self.draw_text(pm, lx, ly, &row.label, &label_style);
                    let lw = self.text_width(FontKind::Annotation, &row.label, POPUP_LINE_PX);
                    let note_style = TextStyle { kind: FontKind::Annotation, px: EVAL_NOTE_PX, color: dim };
                    self.draw_text(pm, lx + lw, ly, &format!(" \u{2014} {note}"), &note_style);
                }
                // A line that cannot be searched: all of it recedes, its
                // reason included, so it reads as accounted for and not as
                // a filter that lost its checkbox.
                (None, Some(_)) => {
                    label_style.color = dim;
                    self.draw_text(pm, lx, ly, &row.text(), &label_style);
                }
                // A derived line ("Physical DPS: 412.6") is a property, not
                // a mod: its name recedes and its number reads, the way the
                // tooltip's own property block is written.
                (Some(i), _) => {
                    let (name, value) = row.label.split_at(i + 1);
                    label_style.color = dim;
                    self.draw_text(pm, lx, ly, name, &label_style);
                    label_style.color = ink;
                    let nw = self.text_width(FontKind::Annotation, name, POPUP_LINE_PX);
                    self.draw_text(pm, lx + nw, ly, value, &label_style);
                }
                (None, None) => self.draw_text(pm, lx, ly, &row.label, &label_style),
            }

            if let Some(score) = row.score {
                // Graded, not gradient: a good roll is gold, a middling one
                // reads as ordinary text, a poor one recedes — and a row the
                // player switched off recedes whatever it rolled.
                let color = if off {
                    dim
                } else if score >= 4.0 {
                    rgb(C_GOLD)
                } else if score >= 2.0 {
                    ink
                } else {
                    dim
                };
                let style = TextStyle { kind: FontKind::Amount, px: EVAL_SCORE_PX, color };
                let text = crate::evaluate_ui::score_text(score);
                let sw = self.text_width(FontKind::Amount, &text, EVAL_SCORE_PX);
                let (l, r) = score_span(g);
                self.draw_text(pm, ax + (l + r - sw) / 2.0, ay + g.score_pos.1 as f32, &text, &style);
            }

            if !filterable {
                continue;
            }
            // Min/max: dark wells, so they read as fields you can type in.
            // The focused one shows the live buffer with a caret and takes a
            // gold border.
            let box_style = TextStyle { kind: FontKind::Amount, px: EVAL_BOX_PX, color: ink };
            // Weapon bounds are open-ended minimums: no max box, so the
            // card cannot suggest an upper bound the search will not send.
            let has_max = row.target.is_some_and(crate::evaluate_ui::Target::has_max);
            for (field, bx, val) in [
                (Field::Min, &g.min_box, Some(row.min.map(fmt_num).unwrap_or_default())),
                (Field::Max, &g.max_box, has_max.then(|| row.max.map(fmt_num).unwrap_or_default())),
            ] {
                let Some(val) = val else { continue };
                let focused = editing == Some((i, field));
                let (bx0, by0) = (ax + bx.x as f32, ay + bx.y as f32);
                let (bw, bh) = (bx.w as f32, bx.h as f32);
                self.pill(pm, bx0, by0, bw, bh, if focused { rgb(C_GOLD) } else { rgb(C_LINE) });
                let mut well = Paint::default();
                well.set_color(rgb(C_WELL));
                if let Some(r) = SkRect::from_xywh(bx0 + 1.5, by0 + 1.5, bw - 3.0, bh - 3.0) {
                    pm.fill_rect(r, &well, self.xf(), None);
                }
                let shown = if focused { format!("{edit_buf}_") } else { val };
                self.draw_text(pm, bx0 + 5.0, by0 + bh - 5.0, &shown, &box_style);
            }
        }

        // "Show N more" / "Hide": a quiet outlined link, not a call to action.
        if let Some((rect, label)) = &lay.hidden_toggle {
            let (bx, by) = (ax + rect.x as f32, ay + rect.y as f32);
            let (bw, bh) = (rect.w as f32, rect.h as f32);
            self.pill(pm, bx, by, bw, bh, rgb(C_LINE));
            let style = TextStyle { kind: FontKind::Amount, px: 13.0, color: dim };
            let tw = self.text_width(FontKind::Amount, label, 13.0);
            self.draw_text(pm, bx + (bw - tw).max(0.0) / 2.0, by + bh - 6.0, label, &style);
        }

        // Strictness: two radios inside the rects the layout hands back — a
        // marker square at the rect's left edge, its label beside it. The
        // chosen one takes the gold border and a faint gold wash; the other
        // stays a hairline, so which mode is armed reads without reading.
        for (rect, s) in &lay.strictness {
            let selected = *s == panel.strictness;
            let (bx, by) = (ax + rect.x as f32, ay + rect.y as f32);
            let (bw, bh) = (rect.w as f32, rect.h as f32);
            self.pill(pm, bx, by, bw, bh, if selected { rgb(C_GOLD) } else { rgb(C_LINE) });
            if selected {
                let mut lift = Paint::default();
                lift.set_color(Color::from_rgba8(C_GOLD.0, C_GOLD.1, C_GOLD.2, 0x2E));
                lift.anti_alias = true;
                if let Some(r) = SkRect::from_xywh(bx + 1.0, by + 1.0, bw - 2.0, bh - 2.0) {
                    pm.fill_rect(r, &lift, self.xf(), None);
                }
            }
            let side = (bh - 10.0).max(6.0);
            let mx = bx + 6.0;
            let my = by + (bh - side) / 2.0;
            self.pill(pm, mx, my, side, side, if selected { rgb(C_GOLD) } else { dim });
            if selected {
                let mut inner = Paint::default();
                inner.set_color(rgb(C_GOLD));
                if let Some(r) = SkRect::from_xywh(mx + 3.0, my + 3.0, side - 6.0, side - 6.0) {
                    pm.fill_rect(r, &inner, self.xf(), None);
                }
            }
            let style = TextStyle {
                kind: FontKind::Amount,
                px: 14.0,
                color: if selected { ink } else { dim },
            };
            self.draw_text(pm, mx + side + 6.0, by + bh - 6.0, s.label(), &style);
        }

        for (rect, action, label) in &lay.buttons {
            let (bx, by) = (ax + rect.x as f32, ay + rect.y as f32);
            // Search reads as switched off while one is running: a press
            // on it is not sent (see `Panel::searching`).
            let off = panel.searching && *action == crate::evaluate_ui::Action::Search;
            let border = if off { rgb(C_LINE) } else { border_color(Tier::Jackpot) };
            self.pill(pm, bx, by, rect.w as f32, rect.h as f32, border);
            let style = TextStyle { kind: FontKind::Amount, px: 16.0, color: if off { dim } else { ink } };
            self.draw_text(pm, bx + 12.0, by + rect.h as f32 - 8.0, label, &style);
        }
        if !panel.status.is_empty() {
            let style = TextStyle { kind: FontKind::Annotation, px: 15.0, color: dim };
            self.draw_text(pm, ax + lay.status_pos.0 as f32, ay + lay.status_pos.1 as f32, &panel.status, &style);
        }
        // The request budget, red when the next few searches would hit the
        // cap.
        if let Some(pos) = lay.budget_pos {
            let style =
                TextStyle { kind: FontKind::Annotation, px: 15.0, color: if f.budget_low { stale_color() } else { dim } };
            self.draw_text(pm, ax + pos.0 as f32, ay + pos.1 as f32, &f.budget_text, &style);
        }

        let row_face = TextStyle { kind: FontKind::Annotation, px: 16.0, color: ink };
        let note_face = TextStyle { kind: FontKind::Annotation, px: 14.0, color: dim };

        // poe.ninja: the price, the direction in the market view's colours,
        // then the band, the volume and the caveat in one dim line.
        if let (Some(g), Some(n)) = (&lay.ninja, &f.ninja) {
            self.section(pm, ax + g.head_pos.0 as f32, ay + g.head_pos.1 as f32, "poe.ninja");
            let price = TextStyle { kind: FontKind::Annotation, px: POPUP_LINE_PX, color: ink };
            let (px_, py) = (ax + g.price_pos.0 as f32, ay + g.price_pos.1 as f32);
            self.draw_text(pm, px_, py, &n.price, &price);
            let moved = match n.direction.as_str() {
                "rising" => rgb(C_UP),
                "falling" => rgb(C_DOWN),
                _ => dim,
            };
            let pw = self.text_width(FontKind::Annotation, &n.price, POPUP_LINE_PX);
            self.draw_text(pm, px_ + pw + 12.0, py, &n.direction, &TextStyle { color: moved, ..price });
            let detail = crate::evaluate_ui::ninja_detail(n);
            self.draw_text(pm, ax + g.detail_pos.0 as f32, ay + g.detail_pos.1 as f32, &detail, &note_face);
        }
        if let Some(pos) = lay.ladder_pos {
            self.draw_text(pm, ax + pos.0 as f32, ay + pos.1 as f32, &f.ladder, &note_face);
        }
        // Likely price-fixed: a red-bordered strip with the numbers and
        // the button that searches in exalted and divine only.
        if let (Some(g), Some(p)) = (&lay.price_fixed, &f.price_fixed) {
            let s = &g.strip;
            self.pill(pm, ax + s.x as f32, ay + s.y as f32, s.w as f32, s.h as f32, stale_color());
            let style = TextStyle { kind: FontKind::Annotation, px: 15.0, color: ink };
            self.draw_text(pm, ax + g.text_pos.0 as f32, ay + g.text_pos.1 as f32, &p.text, &style);
            self.small_button(pm, ax, ay, &g.button, &p.button, true);
        }

        // The listings table. An offline seller's row recedes whole; the
        // hovered row takes the faint gold wash the selected radio uses.
        if let Some(t) = &lay.table {
            self.captions(pm, (ax, ay), (t.caption_pos.1, &t.cols), &crate::evaluate_ui::LISTING_CAPTIONS, w);
            for (i, (rect, row)) in t.rows.iter().zip(&f.listings).enumerate() {
                let (rx, ry) = (ax + rect.x as f32, ay + rect.y as f32);
                if f.hover == Some(i) {
                    let mut lift = Paint::default();
                    lift.set_color(Color::from_rgba8(C_GOLD.0, C_GOLD.1, C_GOLD.2, 0x2E));
                    if let Some(r) = SkRect::from_xywh(rx, ry, rect.w as f32, rect.h as f32) {
                        pm.fill_rect(r, &lift, self.xf(), None);
                    }
                }
                let baseline = ry + rect.h as f32 - 6.0;
                let offline = row.state == SellerState::Offline;
                let row_ink = if offline { dim } else { ink };
                let cells = row.cells();
                let colors = [
                    row_ink,
                    row_ink,
                    if row.mine && !offline { rgb(C_GOLD) } else { row_ink },
                    match row.state {
                        SellerState::Online => rgb(C_UP),
                        SellerState::Afk => rgb(C_SUFFIX),
                        SellerState::Offline => dim,
                    },
                    dim,
                ];
                for ((cell, x), color) in cells.iter().zip(&t.cols).zip(colors) {
                    self.draw_text(pm, ax + *x as f32, baseline, cell, &TextStyle { color, ..row_face });
                }
            }
        }

        // Closest listings: the lines name listings and prices; the note
        // says how the nearest differs when none is close.
        if let (Some(g), Some(c)) = (&lay.closest, &f.closest) {
            self.section(pm, ax + g.head_pos.0 as f32, ay + g.head_pos.1 as f32, "Closest listings");
            for (pos, line) in g.lines.iter().zip(&g.texts) {
                self.draw_text(pm, ax + pos.0 as f32, ay + pos.1 as f32, line, &row_face);
            }
            if let (Some(pos), Some(n)) = (g.note_pos, &c.nearest) {
                self.draw_text(pm, ax + pos.0 as f32, ay + pos.1 as f32, n, &note_face);
            }
        }

        // What each mod is worth: the button beside the heading, then per
        // mod the two cheapest prices and the listings they came from.
        if let Some(g) = &lay.attribution {
            self.section(pm, ax + g.head_pos.0 as f32, ay + g.head_pos.1 as f32, crate::evaluate_ui::ATTRIBUTE_LABEL);
            self.small_button(pm, ax, ay, &g.button, crate::evaluate_ui::ATTRIBUTE_BUTTON, f.attribute_enabled);
            if let Some(cap) = g.caption_pos {
                self.captions(pm, (ax, ay), (cap.1, &g.cols), &crate::evaluate_ui::ATTRIBUTION_CAPTIONS, w);
            }
            for ((figures, named), a) in g.rows.iter().zip(&f.attribution) {
                let fy = ay + *figures as f32;
                self.draw_text(pm, ax + g.cols[0] as f32, fy, &a.label, &row_face);
                self.draw_text(pm, ax + g.cols[1] as f32, fy, &a.with, &row_face);
                self.draw_text(pm, ax + g.cols[2] as f32, fy, &a.without, &row_face);
                self.draw_text(pm, ax + g.cols[0] as f32, ay + *named as f32, &a.text, &note_face);
            }
        }

        // Bulk offers for a stackable, cheapest first.
        if let (Some(g), Some(b)) = (&lay.bulk, &f.bulk) {
            let t = &g.table;
            self.section(pm, ax + g.head_pos.0 as f32, ay + g.head_pos.1 as f32, "Bulk offers");
            self.captions(pm, (ax, ay), (t.caption_pos.1, &t.cols), &crate::evaluate_ui::BULK_CAPTIONS, w);
            for (rect, o) in t.rows.iter().zip(&b.offers) {
                let baseline = ay + rect.y as f32 + rect.h as f32 - 6.0;
                let color = if o.state == SellerState::Offline { dim } else { ink };
                let style = TextStyle { color, ..row_face };
                for (cell, x) in [&o.have, &o.want, &o.stock, &o.seller].iter().zip(&t.cols) {
                    self.draw_text(pm, ax + *x as f32, baseline, cell, &style);
                }
            }
            if let Some(pos) = g.note_pos {
                self.draw_text(pm, ax + pos.0 as f32, ay + pos.1 as f32, &b.note, &note_face);
            }
        }

        if let (Some(g), Some(row)) = (&lay.card, f.hover.and_then(|i| f.listings.get(i))) {
            self.draw_listing_card(pm, &row.card, g, anchor);
        }
    }

    /// The hover card: the listing's item as its tooltip would show it,
    /// the name in its rarity's colour, the base and rarity under it, the
    /// computed figures, then every mod line with its tier badges in the
    /// gutter (prefix blue, suffix amber, as on the item card's gutter)
    /// and its text coloured by the mod's domain.
    fn draw_listing_card(
        &self,
        pm: &mut Pixmap,
        card: &crate::evaluate_ui::Card,
        g: &crate::evaluate_ui::CardGeom,
        anchor: (i32, i32),
    ) {
        use crate::evaluate_ui::LineKind;
        let (ax, ay) = (anchor.0 as f32, anchor.1 as f32);
        let ink = rgb(C_INK);
        let dim = rgb(C_INK2);
        let r = &g.rect;
        self.pill(pm, ax + r.x as f32, ay + r.y as f32, r.w as f32, r.h as f32, panel_border());
        let title = TextStyle { kind: FontKind::Amount, px: 20.0, color: Self::rarity_color(&card.rarity) };
        self.draw_text(pm, ax + g.title_pos.0 as f32, ay + g.title_pos.1 as f32, crate::evaluate_ui::card_title(card), &title);
        if let Some(pos) = g.base_pos {
            let style = TextStyle { kind: FontKind::Amount, px: EVAL_PROP_PX, color: dim };
            self.draw_text(pm, ax + pos.0 as f32, ay + pos.1 as f32, &card.base, &style);
        }
        let prop_x = ax + g.rarity_pos.0 as f32;
        self.draw_prop_line(pm, prop_x, ay + g.rarity_pos.1 as f32, &format!("Rarity: {}", card.rarity), EVAL_PROP_PX);
        for (pos, (label, value)) in g.figures.iter().zip(&card.figures) {
            self.draw_prop_line(pm, ax + pos.0 as f32, ay + pos.1 as f32, &format!("{label}: {value}"), EVAL_PROP_PX);
        }
        for &y in &g.rules {
            self.pill(pm, ax + r.x as f32 + 12.0, ay + y as f32, r.w as f32 - 24.0, 1.0, rgb(C_LINE));
        }
        let badge = TextStyle { kind: FontKind::Amount, px: EVAL_BADGE_PX, color: dim };
        let text = TextStyle { kind: FontKind::Annotation, px: POPUP_LINE_PX, color: ink };
        for (baseline, line) in g.lines.iter().zip(&card.lines) {
            let y = ay + *baseline as f32;
            let mut bx = ax + g.badge_x as f32;
            for tier in &line.tiers {
                let color = match tier.chars().next() {
                    Some('P') => rgb(C_BLUE_LT),
                    Some('S') => rgb(C_SUFFIX),
                    _ => dim,
                };
                self.draw_text(pm, bx, y, tier, &TextStyle { color, ..badge });
                bx += self.text_width(FontKind::Amount, tier, EVAL_BADGE_PX) + 4.0;
            }
            let color = match line.kind {
                LineKind::Implicit | LineKind::Enchant | LineKind::Crafted => rgb(C_BLUE_LT),
                LineKind::Rune => rgb(C_SUFFIX),
                LineKind::Desecrated | LineKind::Mutated => rgb(C_DOWN),
                LineKind::Fractured => rgb(C_GOLD),
                LineKind::Pseudo => dim,
                LineKind::Explicit => ink,
            };
            self.draw_text(pm, ax + g.text_x as f32, y, &line.text, &TextStyle { color, ..text });
        }
    }

    /// One selectable pill of a choice group (the market panel's tabs):
    /// gold border and a brighter fill when selected, muted hairline
    /// otherwise.
    fn choice_pill(
        &self,
        pm: &mut Pixmap,
        ax: f32,
        ay: f32,
        rect: &crate::config::Rect,
        label: &str,
        selected: bool,
    ) {
        let (px_, py) = (ax + rect.x as f32, ay + rect.y as f32);
        let (pw, ph) = (rect.w as f32, rect.h as f32);
        self.pill(pm, px_, py, pw, ph, if selected { rgb(C_GOLD) } else { rgb(C_LINE) });
        if selected {
            let mut lift = Paint::default();
            lift.set_color(Color::from_rgba8(C_GOLD.0, C_GOLD.1, C_GOLD.2, 0x2E));
            lift.anti_alias = true;
            if let Some(r) = SkRect::from_xywh(px_ + 1.0, py + 1.0, pw - 2.0, ph - 2.0) {
                pm.fill_rect(r, &lift, self.xf(), None);
            }
        }
        let style = TextStyle {
            kind: FontKind::Amount,
            px: 13.0,
            color: if selected { rgb(C_INK) } else { rgb(C_INK2) },
        };
        self.draw_text(pm, px_ + 8.0, py + ph - 5.0, label, &style);
    }

    /// Whether every visible character of `text` has an outline in the
    /// annotation face. A font can map a character to an empty glyph, which
    /// draws as a gap and says nothing about it.
    pub fn can_draw(&self, text: &str) -> bool {
        text.chars().filter(|c| !c.is_whitespace()).all(|c| {
            let (metrics, bitmap) = self.annotation_font.rasterize(c, 18.0);
            self.annotation_font.lookup_glyph_index(c) != 0
                && metrics.width > 0
                && bitmap.iter().any(|cov| *cov > 0)
        })
    }

    /// A week of points as a small line: drawn through the days that have
    /// a figure, broken where one is missing. A lone figure between two
    /// gaps is a dot. Nothing is drawn for a series without two figures.
    fn sparkline(&self, pm: &mut Pixmap, area: (f32, f32, f32, f32), points: &[Option<f64>], color: Color) {
        let (x, y, w, h) = area;
        let known: Vec<f64> = points.iter().flatten().copied().collect();
        if known.len() < 2 || points.len() < 2 {
            return;
        }
        let (lo, hi) = known.iter().fold((f64::MAX, f64::MIN), |(lo, hi), v| (lo.min(*v), hi.max(*v)));
        let span = (hi - lo).max(1e-9);
        let step = w / (points.len() - 1) as f32;
        let at = |i: usize, v: f64| (x + step * i as f32, y + h - ((v - lo) / span) as f32 * h);
        let mut paint = Paint::default();
        paint.set_color(color);
        paint.anti_alias = true;
        let stroke = Stroke { width: 1.5, ..Default::default() };
        let mut run: Vec<(f32, f32)> = Vec::new();
        let flush = |run: &mut Vec<(f32, f32)>, pm: &mut Pixmap| {
            match run.as_slice() {
                [] => {}
                [(px, py)] => {
                    if let Some(dot) = PathBuilder::from_circle(*px, *py, 1.5) {
                        pm.fill_path(&dot, &paint, tiny_skia::FillRule::Winding, self.xf(), None);
                    }
                }
                [first, rest @ ..] => {
                    let mut pb = PathBuilder::new();
                    pb.move_to(first.0, first.1);
                    for (px, py) in rest {
                        pb.line_to(*px, *py);
                    }
                    if let Some(path) = pb.finish() {
                        pm.stroke_path(&path, &paint, &stroke, self.xf(), None);
                    }
                }
            }
            run.clear();
        };
        for (i, p) in points.iter().enumerate() {
            match p {
                Some(v) => run.push(at(i, *v)),
                None => flush(&mut run, pm),
            }
        }
        flush(&mut run, pm);
    }

    /// Draws the market panel from the SAME layout the click handler
    /// hit-tests against (market_ui::layout), offset to `anchor`. A table
    /// older than three hours is drawn in the muted ink throughout; thin
    /// rows are always muted, so they read as background under the liquid
    /// ones.
    /// Band, note, basis and footer text: `market_ui::SMALL_RATIO` of the
    /// row text, the ratio the layout sized their columns with.
    const MARKET_SMALL_PX: f32 = POPUP_LINE_PX * crate::market_ui::SMALL_RATIO;

    pub fn draw_market(
        &self,
        pm: &mut Pixmap,
        p: &crate::market_ui::Panel,
        lay: &crate::market_ui::Layout,
        anchor: (i32, i32),
    ) {
        use crate::market_ui::{Line, Tone};
        let (ax, ay) = (anchor.0 as f32, anchor.1 as f32);
        let greyed = p.greyed();
        let dim = rgb(C_INK2);
        let ink = if greyed { dim } else { rgb(C_INK) };
        let up = if greyed { dim } else { rgb(C_UP) };
        let down = if greyed { dim } else { rgb(C_DOWN) };
        let signed = |sign: i8| match sign {
            1 => up,
            -1 => down,
            _ => ink,
        };
        self.pill(pm, ax, ay, lay.w as f32, lay.h as f32, panel_border());

        let title = TextStyle { kind: FontKind::Amount, px: POPUP_TITLE_PX, color: rgb(C_INK) };
        self.draw_text(pm, ax + lay.title_pos.0 as f32, ay + lay.title_pos.1 as f32, "Market", &title);
        let x_style = TextStyle { kind: FontKind::Amount, px: 18.0, color: rgb(C_INK) };
        self.draw_text(pm, ax + lay.close.x as f32 + 4.0, ay + lay.close.y as f32 + 15.0, "x", &x_style);
        let f = &p.fresh;
        let aged = p.tab != crate::market_ui::Tab::MyRuns
            && (f.stale || f.uniques_stale || greyed || p.uniques_greyed());
        let status_color = if aged { stale_color() } else { dim };
        let status = TextStyle { kind: FontKind::Annotation, px: 14.0, color: status_color };
        self.draw_text(pm, ax + lay.status_pos.0 as f32, ay + lay.status_pos.1 as f32, &p.status(), &status);

        for (rect, tab) in &lay.tabs {
            let label = crate::market_ui::TABS.iter().find(|(t, _)| t == tab).map(|(_, l)| *l).unwrap_or("");
            self.choice_pill(pm, ax, ay, rect, label, *tab == p.tab);
        }
        let arrow = TextStyle { kind: FontKind::Amount, px: 16.0, color: rgb(C_INK) };
        for (rect, glyph) in [(&lay.back, "<"), (&lay.prev, "<"), (&lay.next, ">")] {
            let Some(rect) = rect else { continue };
            let (bx, by) = (ax + rect.x as f32, ay + rect.y as f32);
            self.pill(pm, bx, by, rect.w as f32, rect.h as f32, rgb(C_LINE));
            self.draw_text(pm, bx + 7.0, by + rect.h as f32 - 5.0, glyph, &arrow);
        }
        if let Some(pos) = lay.heading_pos {
            let style = TextStyle { kind: FontKind::Amount, px: 18.0, color: rgb(C_INK) };
            self.draw_text(pm, ax + pos.0 as f32, ay + pos.1 as f32, &crate::market_ui::heading_text(p), &style);
        }
        if let Some(pos) = lay.page_pos {
            let style = TextStyle { kind: FontKind::Annotation, px: 13.0, color: dim };
            self.draw_text(pm, ax + pos.0 as f32, ay + pos.1 as f32, &crate::market_ui::page_text(p), &style);
        }

        let c = &lay.cols;
        let captions = crate::market_ui::captions_for(p);
        let groups = captions.len() == crate::market_ui::GROUP_CAPTIONS.len();
        for (rect, line) in &lay.lines {
            let top = ay + rect.y as f32;
            let baseline = top + rect.h as f32 - 5.0;
            match line {
                Line::Section(title) => {
                    let style = TextStyle { kind: FontKind::Amount, px: 15.0, color: rgb(C_GOLD) };
                    self.draw_text(pm, ax + rect.x as f32, baseline, title, &style);
                }
                Line::Message(text) => {
                    let style = TextStyle { kind: FontKind::Annotation, px: 15.0, color: dim };
                    self.draw_text(pm, ax + rect.x as f32, baseline, text, &style);
                }
                Line::Caption => {
                    let style = TextStyle { kind: FontKind::Amount, px: EVAL_COL_PX, color: dim };
                    let xs: &[i32] = if groups {
                        &[c.price, c.volume, c.change, c.spark]
                    } else {
                        &[c.price, c.volume, c.change, c.band, c.spark]
                    };
                    for (x, caption) in xs.iter().zip(captions) {
                        self.draw_text(pm, ax + *x as f32, baseline, &caption.to_uppercase(), &style);
                    }
                    self.pill(pm, ax + rect.x as f32, top + rect.h as f32 - 1.0, rect.w as f32, 1.0, rgb(C_LINE));
                }
                Line::Note(text) => {
                    let style = TextStyle { kind: FontKind::Annotation, px: Self::MARKET_SMALL_PX, color: dim };
                    self.draw_text(pm, ax + rect.x as f32, baseline, text, &style);
                }
                Line::Cells { table, cells } => {
                    use crate::market_ui::Ink;
                    let Some(xs) = lay.tables.get(*table) else { continue };
                    let is_caption = cells.iter().any(|(_, i)| *i == Ink::Caption);
                    for ((text, cell_ink), x) in cells.iter().zip(xs) {
                        let x = ax + *x as f32;
                        if *cell_ink == Ink::Caption {
                            let style = TextStyle { kind: FontKind::Amount, px: EVAL_COL_PX, color: dim };
                            self.draw_text(pm, x, baseline, &text.to_uppercase(), &style);
                            continue;
                        }
                        // The own-runs figures do not age with the price
                        // table, so its greying does not reach them.
                        let color = match cell_ink {
                            Ink::Dim | Ink::Caption => dim,
                            Ink::Up => rgb(C_UP),
                            Ink::Down => rgb(C_DOWN),
                            Ink::Normal => rgb(C_INK),
                        };
                        let style = TextStyle { kind: FontKind::Annotation, px: POPUP_LINE_PX, color };
                        self.draw_text(pm, x, baseline, text, &style);
                    }
                    if is_caption {
                        self.pill(pm, ax + rect.x as f32, top + rect.h as f32 - 1.0, rect.w as f32, 1.0, rgb(C_LINE));
                    }
                }
                Line::Group(g) => {
                    let first = top + 17.0;
                    let text = TextStyle { kind: FontKind::Annotation, px: POPUP_LINE_PX, color: ink };
                    self.draw_text(pm, ax + c.name as f32, first, &g.label, &text);
                    self.draw_text(pm, ax + c.price as f32, first, &g.volume, &text);
                    self.draw_text(pm, ax + c.volume as f32, first, &g.share, &text);
                    let index = TextStyle { color: signed(g.index_sign), ..text };
                    self.draw_text(pm, ax + c.change as f32, first, &g.index, &index);
                    let area = (ax + c.spark as f32, top + 5.0, crate::market_ui::SPARK_W as f32, 13.0);
                    self.sparkline(pm, area, &g.spark, signed(g.index_sign));
                    let basis = TextStyle { kind: FontKind::Annotation, px: Self::MARKET_SMALL_PX, color: dim };
                    self.draw_text(pm, ax + c.name as f32, top + 34.0, &g.basis, &basis);
                }
                Line::Item(r) => {
                    // A row is as fresh as the part it came from.
                    let row_old = if r.listed { p.uniques_greyed() } else { greyed };
                    let muted = r.tone != Tone::Normal || row_old;
                    let row_ink = if muted { dim } else { rgb(C_INK) };
                    let text = TextStyle { kind: FontKind::Annotation, px: POPUP_LINE_PX, color: row_ink };
                    self.draw_text(pm, ax + c.name as f32, baseline, &r.name, &text);
                    self.draw_text(pm, ax + c.price as f32, baseline, &r.price, &text);
                    self.draw_text(pm, ax + c.volume as f32, baseline, &r.volume, &text);
                    let moved = match r.change_sign {
                        _ if muted => dim,
                        1 => rgb(C_UP),
                        -1 => rgb(C_DOWN),
                        _ => row_ink,
                    };
                    self.draw_text(pm, ax + c.change as f32, baseline, &r.change, &TextStyle { color: moved, ..text });
                    let small = TextStyle { kind: FontKind::Annotation, px: Self::MARKET_SMALL_PX, color: dim };
                    self.draw_text(pm, ax + c.band as f32, baseline, &r.band, &small);
                    let area =
                        (ax + c.spark as f32, top + 4.0, crate::market_ui::SPARK_W as f32, rect.h as f32 - 9.0);
                    self.sparkline(pm, area, &r.spark, moved);
                    self.draw_text(pm, ax + c.note as f32, baseline, &r.note, &small);
                }
            }
        }
        if let Some((pos, text)) = &lay.footer {
            let style = TextStyle { kind: FontKind::Annotation, px: Self::MARKET_SMALL_PX, color: dim };
            self.draw_text(pm, ax + pos.0 as f32, ay + pos.1 as f32, text, &style);
        }
    }

    /// Small text of the craft panel: `craft_ui::SMALL_RATIO` of the body
    /// text, the ratio its layout measured with.
    const CRAFT_SMALL_PX: f32 = POPUP_LINE_PX * crate::craft_ui::SMALL_RATIO;

    /// Draws the craft planner panel from the SAME layout the click handler
    /// hit-tests against (craft_ui::layout), offset to `anchor`. Every
    /// position comes from `lay`.
    pub fn draw_craft(
        &self,
        pm: &mut Pixmap,
        p: &crate::craft_ui::Panel,
        lay: &crate::craft_ui::Layout,
        anchor: (i32, i32),
    ) {
        use crate::craft_ui::{Ink, Style, TABS};
        let (ax, ay) = (anchor.0 as f32, anchor.1 as f32);
        self.pill(pm, ax, ay, lay.w as f32, lay.h as f32, panel_border());

        let title = TextStyle { kind: FontKind::Amount, px: POPUP_TITLE_PX, color: rgb(C_INK) };
        self.draw_text(pm, ax + lay.title_pos.0 as f32, ay + lay.title_pos.1 as f32, "Craft", &title);
        let x_style = TextStyle { kind: FontKind::Amount, px: 18.0, color: rgb(C_INK) };
        self.draw_text(pm, ax + lay.close.x as f32 + 4.0, ay + lay.close.y as f32 + 15.0, "x", &x_style);
        for (rect, view) in &lay.tabs {
            let label = TABS.iter().find(|(v, _)| v == view).map(|(_, l)| *l).unwrap_or("");
            self.choice_pill(pm, ax, ay, rect, label, *view == p.view);
        }

        // The open row sits on a faint lift so its steps read as its own.
        for (rect, _, open) in &lay.rows {
            if *open {
                let mut lift = Paint::default();
                lift.set_color(Color::from_rgba8(C_GOLD.0, C_GOLD.1, C_GOLD.2, 0x18));
                if let Some(r) = SkRect::from_xywh(ax + rect.x as f32, ay + rect.y as f32, rect.w as f32, rect.h as f32) {
                    pm.fill_rect(r, &lift, self.xf(), None);
                }
            }
        }

        let color = |ink: Ink| match ink {
            Ink::Normal => rgb(C_INK),
            Ink::Dim => rgb(C_INK2),
            Ink::Gold => rgb(C_GOLD),
            Ink::Magic => rgb(C_BLUE_LT),
            Ink::Warn => rgb(C_DOWN),
            Ink::Unknown => rgb(C_SUFFIX),
        };
        for t in std::iter::once(&lay.status).chain(&lay.texts) {
            let (kind, px) = match t.style {
                Style::Body => (FontKind::Annotation, POPUP_LINE_PX),
                Style::Small => (FontKind::Annotation, Self::CRAFT_SMALL_PX),
                Style::Section => (FontKind::Amount, 15.0),
                // A shade under the body size: small capitals run a little
                // wider than the face the layout measured with.
                Style::Heading => (FontKind::Amount, 17.0),
            };
            let baseline = ay + (t.rect.y + t.rect.h as i32 - 5) as f32;
            self.draw_text(pm, ax + t.rect.x as f32, baseline, &t.text, &TextStyle { kind, px, color: color(t.ink) });
        }

        let arrow = TextStyle { kind: FontKind::Amount, px: 14.0, color: rgb(C_INK) };
        let tier = TextStyle { kind: FontKind::Annotation, px: POPUP_LINE_PX, color: rgb(C_INK) };
        for f in &lay.families {
            if !f.greyed {
                self.eval_check(pm, &f.check, ax, ay, f.ticked);
            }
            let Some(t) = &f.tier else { continue };
            for (rect, glyph) in [(&t.better, "<"), (&t.worse, ">")] {
                let (bx, by) = (ax + rect.x as f32, ay + rect.y as f32);
                self.pill(pm, bx, by, rect.w as f32, rect.h as f32, rgb(C_LINE));
                let gw = self.text_width(FontKind::Amount, glyph, 14.0);
                self.draw_text(pm, bx + (rect.w as f32 - gw) / 2.0, by + rect.h as f32 - 5.0, glyph, &arrow);
            }
            let baseline = ay + (t.better.y + t.better.h as i32 - 3) as f32;
            self.draw_text(pm, ax + t.text_x as f32, baseline, &t.text, &tier);
        }
        for b in &lay.buttons {
            self.small_button(pm, ax, ay, &b.rect, &b.label, b.on);
        }
    }

    /// Pixel size the popup pill will occupy, for placement and the
    /// move-away inside test. Must mirror draw_popup's layout math.
    /// Width the popup needs for its widest line, floored at
    /// POPUP_MIN_WIDTH: the box accommodates the text, never the reverse.
    fn popup_width(&self, popup: &Popup) -> f32 {
        let mut w = self.text_width(FontKind::Amount, &popup.title, POPUP_TITLE_PX);
        for line in &popup.lines {
            let mut lw = self.text_width(FontKind::Annotation, &line.text, POPUP_LINE_PX);
            if self.icon_for(line.denom).is_some() {
                lw += ICON_GAP + ICON_SIZE as f32;
            }
            w = w.max(lw);
        }
        (w + POPUP_PAD * 2.0).max(POPUP_MIN_WIDTH)
    }

    pub fn popup_size(&self, popup: &Popup) -> (i32, i32) {
        let title_h = POPUP_TITLE_PX + POPUP_ROW_GAP;
        let line_h = POPUP_LINE_PX + POPUP_ROW_GAP;
        let content_h = title_h + popup.lines.len() as f32 * line_h;
        let pill_h = content_h + POPUP_PAD * 2.0 - POPUP_ROW_GAP;
        (self.popup_width(popup).ceil() as i32, pill_h.ceil() as i32)
    }

    /// Draws the hover price-check popup: same parchment pill language as
    /// the row labels, but a single fixed-width block with a title line
    /// (item name) followed by one priced line per `popup.lines`, each with
    /// its currency icon composited the same way as `draw_frame`'s rows.
    /// `anchor` is the popup's top-left corner in surface-local pixels.
    pub fn draw_popup(&self, pm: &mut Pixmap, popup: &Popup, anchor: (i32, i32)) {
        let (ax, ay) = anchor;
        let title_h = POPUP_TITLE_PX + POPUP_ROW_GAP;
        let line_h = POPUP_LINE_PX + POPUP_ROW_GAP;
        let content_h = title_h + popup.lines.len() as f32 * line_h;
        let pill_h = content_h + POPUP_PAD * 2.0 - POPUP_ROW_GAP;
        let pill_x = ax as f32;
        let pill_y = ay as f32;
        self.pill(pm, pill_x, pill_y, self.popup_width(popup), pill_h, panel_border());

        let title_color = rgb(C_INK);
        let title_style = TextStyle { kind: FontKind::Amount, px: POPUP_TITLE_PX, color: title_color };
        let title_baseline = pill_y + POPUP_PAD + POPUP_TITLE_PX * 0.8;
        self.draw_text(pm, pill_x + POPUP_PAD, title_baseline, &popup.title, &title_style);

        let mut row_top = pill_y + POPUP_PAD + title_h;
        for line in &popup.lines {
            let icon = self.icon_for(line.denom);
            let line_style = TextStyle { kind: FontKind::Annotation, px: POPUP_LINE_PX, color: title_color };
            let baseline = row_top + POPUP_LINE_PX * 0.8;
            let mut pen_x = pill_x + POPUP_PAD;
            self.draw_text(pm, pen_x, baseline, &line.text, &line_style);
            if icon.is_some() {
                pen_x += self.text_width(FontKind::Annotation, &line.text, POPUP_LINE_PX) + ICON_GAP;
                let icon_y = (row_top + POPUP_LINE_PX / 2.0 - ICON_SIZE as f32 / 2.0) as i32;
                self.composite_icon(pm, line.denom, pen_x.round() as i32, icon_y);
            }
            row_top += line_h;
        }
    }
}

fn load_icon(bytes: &[u8]) -> anyhow::Result<RgbaImage> {
    Ok(image::load_from_memory(bytes)?.to_rgba8())
}
