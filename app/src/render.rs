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

fn rgb(c: (u8, u8, u8)) -> Color {
    Color::from_rgba8(c.0, c.1, c.2, 0xFF)
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

/// Neutral bronze hairline for container panels (popup, appraisal, total),
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

    /// Rendered pixel width of an appraisal row label, so the panel layout can
    /// size the mod-text column against the exact glyphs it will draw (matching
    /// `draw_appraisal`'s font and size). Used by both the renderer and the
    /// main loop's hit-test so their geometry stays identical.
    pub fn appraisal_label_width(&self, text: &str) -> i32 {
        self.text_width(FontKind::Annotation, text, POPUP_LINE_PX).ceil() as i32
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

            let pill_h = AMOUNT_PX + 10.0;
            let content_w = amount_w + icon_w + old_w;
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

    /// Draws the hover price-check popup: same parchment pill language as
    /// the row labels, but a single fixed-width block with a title line
    /// (item name) followed by one priced line per `popup.lines`, each with
    /// its currency icon composited the same way as `draw_frame`'s rows.
    /// `anchor` is the popup's top-left corner in surface-local pixels.
    /// Draws the interactive appraisal panel from the SAME layout the
    /// click handler hit-tests against (appraise_ui::layout), offset to
    /// `anchor` (surface-local top-left).
    #[allow(clippy::too_many_arguments)]
    pub fn draw_appraisal(
        &self,
        pm: &mut Pixmap,
        panel: &crate::appraise_ui::Panel,
        lay: &crate::appraise_ui::Layout,
        anchor: (i32, i32),
        editing: Option<(usize, crate::appraise_ui::Field)>,
        edit_buf: &str,
    ) {
        use crate::appraise_ui::Field;
        let (ax, ay) = (anchor.0 as f32, anchor.1 as f32);
        let ink = rgb(C_INK);
        let dim = rgb(C_INK2);
        self.pill(pm, ax, ay, lay.size.0 as f32, lay.size.1 as f32, panel_border());

        let title_style = TextStyle { kind: FontKind::Amount, px: POPUP_TITLE_PX, color: ink };
        self.draw_text(pm, ax + 12.0, ay + 12.0 + POPUP_TITLE_PX * 0.8, &panel.title, &title_style);
        // Close X.
        let x_style = TextStyle { kind: FontKind::Amount, px: 18.0, color: ink };
        self.draw_text(
            pm,
            ax + lay.close.x as f32 + 4.0,
            ay + lay.close.y as f32 + 15.0,
            "x",
            &x_style,
        );

        // Base-type toggle row: a checkbox + the base name. Unchecked means a
        // mods-only search across every base.
        if let (Some(check), Some(base)) = (&lay.base_check, &panel.base) {
            let (cx, cy) = (ax + check.x as f32, ay + check.y as f32);
            let side = check.w as f32;
            let color = if base.enabled { ink } else { dim };
            self.pill(pm, cx, cy, side, side, color);
            if base.enabled {
                let mut inner = tiny_skia::Paint::default();
                inner.set_color(ink);
                if let Some(r) = tiny_skia::Rect::from_xywh(cx + 4.0, cy + 4.0, side - 8.0, side - 8.0) {
                    pm.fill_rect(r, &inner, tiny_skia::Transform::identity(), None);
                }
            }
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
        }

        // Tag chip abbreviation + colour by group.
        let chip = |tag: &str| -> (&'static str, Color) {
            match tag {
                "implicit" => ("impl", rgb(C_BLUE_LT)),
                "explicit" => ("expl", rgb(C_INK2)),
                _ => ("map", Color::from_rgba8(0xD9, 0x9A, 0x4A, 0xFF)),
            }
        };
        let tag_style_px = 13.0;
        for (g, m) in lay.rows.iter().zip(&panel.mods) {
            // Separator line above the first row of each group.
            if g.group_start {
                let sep = rgb(C_LINE);
                self.pill(pm, ax + 10.0, ay + g.check.y as f32 - 7.0, lay.size.0 as f32 - 20.0, 1.0, sep);
            }
            let (cx, cy) = (ax + g.check.x as f32, ay + g.check.y as f32);
            let side = g.check.w as f32;
            // Checkbox: outline always, filled square when enabled.
            let color = if m.enabled { ink } else { dim };
            self.pill(pm, cx, cy, side, side, color);
            if m.enabled {
                let mut inner = tiny_skia::Paint::default();
                inner.set_color(ink);
                if let Some(r) = tiny_skia::Rect::from_xywh(cx + 4.0, cy + 4.0, side - 8.0, side - 8.0) {
                    pm.fill_rect(r, &inner, tiny_skia::Transform::identity(), None);
                }
            }
            // Tag chip.
            let (chip_txt, chip_col) = chip(&m.tag);
            let chip_style = TextStyle { kind: FontKind::Amount, px: tag_style_px, color: chip_col };
            self.draw_text(pm, ax + g.tag_pos.0 as f32, ay + g.tag_pos.1 as f32, chip_txt, &chip_style);

            let tier = m.tier.map(|t| format!("T{t} ")).unwrap_or_default();
            let label_style = TextStyle {
                kind: FontKind::Annotation,
                px: POPUP_LINE_PX,
                color: if m.enabled { ink } else { dim },
            };
            self.draw_text(
                pm,
                ax + g.label_pos.0 as f32,
                ay + g.label_pos.1 as f32,
                &format!("{tier}{}", m.label),
                &label_style,
            );
            // Min/max value boxes: outlined, right-aligned; a focused box shows
            // the live edit buffer and a brighter border.
            let box_style = TextStyle { kind: FontKind::Amount, px: 14.0, color: ink };
            for (field, bx, val) in [
                (Field::Min, &g.min_box, crate::appraise_ui::fmt_num(m.min)),
                (
                    Field::Max,
                    &g.max_box,
                    m.max.map(crate::appraise_ui::fmt_num).unwrap_or_default(),
                ),
            ] {
                let focused = editing == Some((m.filter_index, field));
                let border = if focused { rgb(C_GOLD) } else { rgb(C_LINE) };
                self.pill(pm, ax + bx.x as f32, ay + bx.y as f32, bx.w as f32, bx.h as f32, border);
                let shown = if focused { edit_buf.to_string() } else { val };
                self.draw_text(
                    pm,
                    ax + bx.x as f32 + 5.0,
                    ay + bx.y as f32 + bx.h as f32 - 5.0,
                    &shown,
                    &box_style,
                );
            }
        }

        for (rect, _, label) in &lay.buttons {
            let (bx, by) = (ax + rect.x as f32, ay + rect.y as f32);
            self.pill(pm, bx, by, rect.w as f32, rect.h as f32, border_color(Tier::Jackpot));
            let style = TextStyle { kind: FontKind::Amount, px: 16.0, color: ink };
            self.draw_text(pm, bx + 12.0, by + rect.h as f32 - 8.0, label, &style);
        }
        if !panel.status.is_empty() {
            let style = TextStyle { kind: FontKind::Annotation, px: 15.0, color: dim };
            self.draw_text(
                pm,
                ax + lay.status_pos.0 as f32,
                ay + lay.status_pos.1 as f32,
                &panel.status,
                &style,
            );
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
