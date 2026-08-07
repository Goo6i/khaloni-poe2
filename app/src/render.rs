use std::cell::RefCell;
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

// Keyed by (face, char, font size in integer tenths of a pixel).
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
    icon_divine: RgbaImage,
    icon_exalted: RgbaImage,
    icon_chaos: RgbaImage,
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
// Suffix badges: a warm amber against the prefixes' C_BLUE_LT, so the two
// affix families split cool/warm at a glance without inventing a hue
// outside the design system.
const C_SUFFIX: (u8, u8, u8) = (0xD9, 0x9A, 0x4A);
// Dark inset behind an editable value box (same well as the reference
// panel's search field).
const C_WELL: (u8, u8, u8) = (0x12, 0x0D, 0x08);

// Evaluate item-card type scale: a large small-caps name, tooltip-sized
// property lines, and deliberately small gutter type so the mod text stays
// the thing the eye lands on.
const EVAL_NAME_PX: f32 = 24.0;
const EVAL_PROP_PX: f32 = 15.0;
const EVAL_COL_PX: f32 = 11.0;
const EVAL_BADGE_PX: f32 = 13.0;
const EVAL_SCORE_PX: f32 = 14.0;
const EVAL_BOX_PX: f32 = 14.0;

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
            icon_divine: load_icon(ICON_DIVINE_PNG)?,
            icon_exalted: load_icon(ICON_EXALTED_PNG)?,
            icon_chaos: load_icon(ICON_CHAOS_PNG)?,
        })
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
        let mut pen = x;
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
    /// premultiplied) pixmap buffer, `(x, y)` being the icon's top-left.
    fn composite_icon(&self, pm: &mut Pixmap, icon: &RgbaImage, x: i32, y: i32) {
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
        pm.fill_path(&path, &fill_paint, tiny_skia::FillRule::Winding, Transform::identity(), None);

        let mut border_paint = Paint::default();
        border_paint.set_color(border);
        border_paint.anti_alias = true;
        let stroke = Stroke { width: PILL_BORDER_WIDTH, ..Default::default() };
        pm.stroke_path(&path, &border_paint, &stroke, Transform::identity(), None);
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

            if let Some(icon_img) = icon {
                pen_x += ICON_GAP;
                let icon_y = p.y - (ICON_SIZE / 2) as i32;
                self.composite_icon(pm, icon_img, pen_x.round() as i32, icon_y);
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
                pm.fill_rect(rect, &inner, Transform::identity(), None);
            }
        }
    }

    /// Draws the Evaluate item card — the three-column panel that replaced
    /// the flat mod list — from the SAME geometry the click handler
    /// hit-tests against (`evaluate_ui::layout`), offset to `anchor`
    /// (surface-local top-left). Every position comes from `lay`; nothing is
    /// recomputed here, so pixels and click targets cannot drift apart.
    ///
    /// `editing` names the box being typed into as (index into `panel.rows`,
    /// field).
    #[allow(clippy::too_many_arguments)]
    pub fn draw_evaluate(
        &self,
        pm: &mut Pixmap,
        panel: &crate::evaluate_ui::Panel,
        lay: &crate::evaluate_ui::Layout,
        anchor: (i32, i32),
        editing: Option<(usize, crate::evaluate_ui::Field)>,
        edit_buf: &str,
    ) {
        use crate::evaluate_ui::{AffixKind, Field};
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
        let rarity = panel.header.rarity.as_str();
        let name_color = if rarity.eq_ignore_ascii_case("rare") {
            rgb(C_GOLD)
        } else if rarity.eq_ignore_ascii_case("magic") {
            // The readable blue, not the dark border blue: a 24px name in
            // C_BLUE proper disappears into the near-black panel fill.
            rgb(C_BLUE_LT)
        } else {
            ink
        };
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

        // Base-type constraint: unchecked means a mods-only search across
        // every base.
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
            match row.label.find(':').filter(|_| !filterable) {
                // A derived line ("Physical DPS: 412.6") is a property, not
                // a mod: its name recedes and its number reads, the way the
                // tooltip's own property block is written.
                Some(i) => {
                    let (name, value) = row.label.split_at(i + 1);
                    label_style.color = dim;
                    self.draw_text(pm, lx, ly, name, &label_style);
                    label_style.color = ink;
                    let nw = self.text_width(FontKind::Annotation, name, POPUP_LINE_PX);
                    self.draw_text(pm, lx + nw, ly, value, &label_style);
                }
                None => self.draw_text(pm, lx, ly, &row.label, &label_style),
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
            let has_max = matches!(row.target, Some(crate::evaluate_ui::Target::Stat(_)));
            for (field, bx, val) in [
                (Field::Min, &g.min_box, Some(fmt_num(row.min))),
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
                    pm.fill_rect(r, &well, Transform::identity(), None);
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
                    pm.fill_rect(r, &lift, Transform::identity(), None);
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
                    pm.fill_rect(r, &inner, Transform::identity(), None);
                }
            }
            let style = TextStyle {
                kind: FontKind::Amount,
                px: 14.0,
                color: if selected { ink } else { dim },
            };
            self.draw_text(pm, mx + side + 6.0, by + bh - 6.0, s.label(), &style);
        }

        // The answer: heading, the headline number with its orb, then the
        // spread and a reliability word that turns red when the listings
        // disagree too much to trust it.
        if let (Some(rect), Some(est)) = (&lay.estimate_box, &panel.estimate) {
            let (bx, by) = (ax + rect.x as f32, ay + rect.y as f32);
            self.pill(pm, bx, by, rect.w as f32, rect.h as f32, panel_border());
            let head = TextStyle { kind: FontKind::Annotation, px: OLD_PX, color: dim };
            let center = |txt_w: f32| bx + (rect.w as f32 - txt_w) / 2.0;
            let heading = "Estimated Value";
            let hw = self.text_width(FontKind::Annotation, heading, OLD_PX);
            self.draw_text(pm, center(hw), by + 16.0, heading, &head);

            let big = TextStyle { kind: FontKind::Amount, px: AMOUNT_PX, color: rgb(C_GOLD) };
            let aw = self.text_width(FontKind::Amount, &est.amount, AMOUNT_PX);
            let icon = self.icon_for(est.denom);
            let iw = if icon.is_some() { ICON_GAP + ICON_SIZE as f32 } else { 0.0 };
            let ax0 = center(aw + iw);
            self.draw_text(pm, ax0, by + 42.0, &est.amount, &big);
            if let Some(img) = icon {
                self.composite_icon(
                    pm,
                    img,
                    (ax0 + aw + ICON_GAP).round() as i32,
                    (by + 42.0 - AMOUNT_PX * 0.75) as i32,
                );
            }

            // The reliability word carries its own colour, so the line is
            // drawn as three runs rather than one string.
            let sep = "   Reliability: ";
            let detail_w = self.text_width(FontKind::Annotation, &est.detail, OLD_PX);
            let sep_w = self.text_width(FontKind::Annotation, sep, OLD_PX);
            let rel_w = self.text_width(FontKind::Annotation, &est.reliability, OLD_PX);
            let dx = center(detail_w + sep_w + rel_w);
            let rel_style = TextStyle {
                kind: FontKind::Annotation,
                px: OLD_PX,
                color: if est.shaky { rgb(C_RED) } else { ink },
            };
            self.draw_text(pm, dx, by + 62.0, &est.detail, &head);
            self.draw_text(pm, dx + detail_w, by + 62.0, sep, &head);
            self.draw_text(pm, dx + detail_w + sep_w, by + 62.0, &est.reliability, &rel_style);
        }

        for (rect, _, label) in &lay.buttons {
            let (bx, by) = (ax + rect.x as f32, ay + rect.y as f32);
            self.pill(pm, bx, by, rect.w as f32, rect.h as f32, border_color(Tier::Jackpot));
            let style = TextStyle { kind: FontKind::Amount, px: 16.0, color: ink };
            self.draw_text(pm, bx + 12.0, by + rect.h as f32 - 8.0, label, &style);
        }
        if !panel.status.is_empty() {
            let style = TextStyle { kind: FontKind::Annotation, px: 15.0, color: dim };
            self.draw_text(pm, ax + lay.status_pos.0 as f32, ay + lay.status_pos.1 as f32, &panel.status, &style);
        }
        for (pos, line) in lay.listing_pos.iter().zip(&panel.listings) {
            let style = TextStyle { kind: FontKind::Annotation, px: POPUP_LINE_PX, color: ink };
            self.draw_text(pm, ax + pos.0 as f32, ay + pos.1 as f32, line, &style);
        }
    }

    /// Draws the reference-browser panel from the SAME layout the click
    /// handler hit-tests against (reference_ui::layout), offset to `anchor`
    /// (surface-local top-left). All geometry comes from `lay`; nothing is
    /// recomputed here, so pixels and hitboxes cannot drift apart.
    pub fn draw_reference(
        &self,
        pm: &mut Pixmap,
        p: &crate::reference_ui::Panel,
        lay: &crate::reference_ui::Layout,
        anchor: (i32, i32),
    ) {
        let (ax, ay) = (anchor.0 as f32, anchor.1 as f32);
        let ink = rgb(C_INK);
        let dim = rgb(C_INK2);
        self.pill(pm, ax, ay, lay.w as f32, lay.h as f32, panel_border());

        let title_style = TextStyle { kind: FontKind::Amount, px: POPUP_TITLE_PX, color: ink };
        self.draw_text(pm, ax + 12.0, ay + 12.0 + POPUP_TITLE_PX * 0.8, "Reference", &title_style);
        // Close X.
        let x_style = TextStyle { kind: FontKind::Amount, px: 18.0, color: ink };
        self.draw_text(
            pm,
            ax + lay.close.x as f32 + 4.0,
            ay + lay.close.y as f32 + 15.0,
            "x",
            &x_style,
        );

        // Search box: a dark inset behind the query text; a trailing
        // underscore caret shows while the box has keyboard focus.
        let (sx, sy) = (ax + lay.search.x as f32, ay + lay.search.y as f32);
        let (sw, sh) = (lay.search.w as f32, lay.search.h as f32);
        self.pill(pm, sx, sy, sw, sh, rgb(C_LINE));
        let mut inset = Paint::default();
        inset.set_color(Color::from_rgba8(0x12, 0x0D, 0x08, 0xFF));
        if let Some(r) = SkRect::from_xywh(sx + 1.5, sy + 1.5, sw - 3.0, sh - 3.0) {
            pm.fill_rect(r, &inset, Transform::identity(), None);
        }
        let shown = if p.focused { format!("{}_", p.query) } else { p.query.clone() };
        let q_style = TextStyle { kind: FontKind::Annotation, px: 15.0, color: ink };
        self.draw_text(pm, sx + 6.0, sy + sh - 7.0, &shown, &q_style);

        // Category pills: the selected one gets the gold border and a
        // brighter fill; the rest stay muted hairlines.
        for (rect, cat) in &lay.pills {
            let selected = *cat == p.cat;
            let (px_, py) = (ax + rect.x as f32, ay + rect.y as f32);
            let (pw, ph) = (rect.w as f32, rect.h as f32);
            self.pill(pm, px_, py, pw, ph, if selected { rgb(C_GOLD) } else { rgb(C_LINE) });
            if selected {
                let mut lift = Paint::default();
                lift.set_color(Color::from_rgba8(C_GOLD.0, C_GOLD.1, C_GOLD.2, 0x2E));
                lift.anti_alias = true;
                if let Some(r) = SkRect::from_xywh(px_ + 1.0, py + 1.0, pw - 2.0, ph - 2.0) {
                    pm.fill_rect(r, &lift, Transform::identity(), None);
                }
            }
            let style = TextStyle {
                kind: FontKind::Amount,
                px: 13.0,
                color: if selected { ink } else { dim },
            };
            self.draw_text(pm, px_ + 8.0, py + ph - 5.0, crate::reference_ui::cat_label(*cat), &style);
        }

        // Result rows, or the empty-state hint when there is nothing to show.
        if p.rows.is_empty() {
            let msg = if p.query.is_empty() { "type to search" } else { "no matches" };
            let style = TextStyle { kind: FontKind::Annotation, px: 14.0, color: dim };
            let tw = self.text_width(FontKind::Annotation, msg, 14.0);
            self.draw_text(pm, ax + (lay.w as f32 - tw) / 2.0, ay + lay.h as f32 - 6.0, msg, &style);
            return;
        }
        let row_style = TextStyle { kind: FontKind::Annotation, px: POPUP_LINE_PX, color: ink };
        for (rect, text) in lay.rows.iter().zip(&p.rows[lay.visible.clone()]) {
            self.draw_text(
                pm,
                ax + rect.x as f32,
                ay + rect.y as f32 + rect.h as f32 - 5.0,
                text,
                &row_style,
            );
        }
        // Scroll counter when more results exist than fit the window.
        if lay.visible.len() < p.rows.len() {
            let text =
                format!("{}–{} / {}", lay.visible.start + 1, lay.visible.end, p.rows.len());
            let tw = self.text_width(FontKind::Annotation, &text, 13.0);
            let style = TextStyle { kind: FontKind::Annotation, px: 13.0, color: dim };
            self.draw_text(pm, ax + lay.w as f32 - 12.0 - tw, ay + lay.h as f32 - 4.0, &text, &style);
        }
    }

    /// Draws the leveling-checklist panel from the SAME layout the click
    /// handler hit-tests against (leveling_ui::layout), offset to `anchor`
    /// (surface-local top-left). Done steps show a gold-filled checkbox and
    /// muted text; pending steps a hollow box and primary ink.
    pub fn draw_leveling(
        &self,
        pm: &mut Pixmap,
        p: &crate::leveling_ui::Panel,
        lay: &crate::leveling_ui::Layout,
        anchor: (i32, i32),
    ) {
        let (ax, ay) = (anchor.0 as f32, anchor.1 as f32);
        let ink = rgb(C_INK);
        let dim = rgb(C_INK2);
        self.pill(pm, ax, ay, lay.w as f32, lay.h as f32, panel_border());
        // Close X.
        let x_style = TextStyle { kind: FontKind::Amount, px: 18.0, color: ink };
        self.draw_text(
            pm,
            ax + lay.close.x as f32 + 4.0,
            ay + lay.close.y as f32 + 15.0,
            "x",
            &x_style,
        );

        let Some(act) = p.acts.get(p.act) else {
            let msg = "leveling data unavailable";
            let style = TextStyle { kind: FontKind::Annotation, px: 15.0, color: dim };
            let tw = self.text_width(FontKind::Annotation, msg, 15.0);
            self.draw_text(
                pm,
                ax + (lay.w as f32 - tw) / 2.0,
                ay + lay.h as f32 / 2.0 + 15.0 * 0.35,
                msg,
                &style,
            );
            return;
        };

        // Act header: prev/next arrow boxes flanking the centered act title.
        for (rect, glyph) in [(&lay.prev, "<"), (&lay.next, ">")] {
            let (bx, by) = (ax + rect.x as f32, ay + rect.y as f32);
            self.pill(pm, bx, by, rect.w as f32, rect.h as f32, rgb(C_LINE));
            let style = TextStyle { kind: FontKind::Amount, px: 16.0, color: ink };
            self.draw_text(pm, bx + 6.0, by + rect.h as f32 - 5.0, glyph, &style);
        }
        let title = if act.name.is_empty() { format!("Act {}", act.act) } else { act.name.clone() };
        let title_style = TextStyle { kind: FontKind::Amount, px: 18.0, color: ink };
        let tw = self.text_width(FontKind::Amount, &title, 18.0);
        self.draw_text(
            pm,
            ax + (lay.w as f32 - tw) / 2.0,
            ay + lay.prev.y as f32 + lay.prev.h as f32 - 4.0,
            &title,
            &title_style,
        );

        for ((rect, (check, id)), i) in lay.rows.iter().zip(&lay.checks).zip(lay.visible.clone()) {
            let done = p.done.contains(id);
            let (cx, cy) = (ax + check.x as f32, ay + check.y as f32);
            let side = check.w as f32;
            if done {
                let mut gold = Paint::default();
                gold.set_color(rgb(C_GOLD));
                gold.anti_alias = true;
                if let Some(r) = SkRect::from_xywh(cx, cy, side, side) {
                    pm.fill_rect(r, &gold, Transform::identity(), None);
                }
                // Dark check mark over the gold fill (stroked, not a font
                // glyph: Fontin has no U+2713).
                let mut pb = PathBuilder::new();
                pb.move_to(cx + side * 0.22, cy + side * 0.55);
                pb.line_to(cx + side * 0.42, cy + side * 0.75);
                pb.line_to(cx + side * 0.78, cy + side * 0.25);
                if let Some(path) = pb.finish() {
                    let mut mark = Paint::default();
                    mark.set_color(Color::from_rgba8(C_PANEL.0, C_PANEL.1, C_PANEL.2, 0xFF));
                    mark.anti_alias = true;
                    let stroke = Stroke { width: 2.0, ..Default::default() };
                    pm.stroke_path(&path, &mark, &stroke, Transform::identity(), None);
                }
            } else {
                self.pill(pm, cx, cy, side, side, dim);
            }
            let style = TextStyle {
                kind: FontKind::Annotation,
                px: POPUP_LINE_PX,
                color: if done { rgb(C_JUNK_LT) } else { ink },
            };
            let text = crate::leveling_ui::drawn_step(&act.steps[i]);
            self.draw_text(
                pm,
                cx + side + 8.0,
                ay + rect.y as f32 + rect.h as f32 - 5.0,
                &text,
                &style,
            );
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
            if let Some(icon_img) = icon {
                pen_x += self.text_width(FontKind::Annotation, &line.text, POPUP_LINE_PX) + ICON_GAP;
                let icon_y = (row_top + POPUP_LINE_PX / 2.0 - ICON_SIZE as f32 / 2.0) as i32;
                self.composite_icon(pm, icon_img, pen_x.round() as i32, icon_y);
            }
            row_top += line_h;
        }
    }
}

fn load_icon(bytes: &[u8]) -> anyhow::Result<RgbaImage> {
    let img = image::load_from_memory(bytes)?.to_rgba8();
    Ok(image::imageops::resize(&img, ICON_SIZE, ICON_SIZE, image::imageops::FilterType::Lanczos3))
}
