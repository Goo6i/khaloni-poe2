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
const ICON_SIZE: u32 = 24;
const ICON_GAP: f32 = 4.0;
const PILL_PAD_X: f32 = 8.0;
const PILL_CORNER: f32 = 4.0;
const PILL_BORDER_WIDTH: f32 = 1.5;

// Hover price-check popup (Stage A: display-only, no interactivity).
const POPUP_WIDTH: f32 = 320.0;
const POPUP_TITLE_PX: f32 = 22.0;
const POPUP_LINE_PX: f32 = 18.0;
const POPUP_PAD: f32 = 12.0;
const POPUP_ROW_GAP: f32 = 6.0;

// Runeshape-panel parchment.
const PILL_FILL: (u8, u8, u8, u8) = (0xEA, 0xDF, 0xC6, 235);
const PILL_BORDER: (u8, u8, u8) = (0x6B, 0x56, 0x37);
const PILL_BORDER_JACKPOT: (u8, u8, u8) = (0x8A, 0x6A, 0x2F);
const STALE_COLOR: (u8, u8, u8) = (0x8B, 0x3A, 0x2E);

fn amount_color(t: Tier) -> Color {
    match t {
        Tier::Junk | Tier::Unknown => Color::from_rgba8(0x6E, 0x65, 0x5A, 0xFF),
        Tier::Decent => Color::from_rgba8(0x3A, 0x2C, 0x1A, 0xFF),
        Tier::Jackpot => Color::from_rgba8(0x9C, 0x4E, 0x12, 0xFF),
    }
}

fn border_color(t: Tier) -> Color {
    let (r, g, b) = if t == Tier::Jackpot { PILL_BORDER_JACKPOT } else { PILL_BORDER };
    Color::from_rgba8(r, g, b, 0xFF)
}

fn stale_color() -> Color {
    Color::from_rgba8(STALE_COLOR.0, STALE_COLOR.1, STALE_COLOR.2, 0xFF)
}

fn best_border_color() -> Color {
    Color::from_rgba8(0xC9, 0xA2, 0x27, 0xFF)
}

fn pill_fill_color() -> Color {
    Color::from_rgba8(PILL_FILL.0, PILL_FILL.1, PILL_FILL.2, PILL_FILL.3)
}

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
                    border_color(Tier::Decent),
                );
                let color = if stale { stale_color() } else { Color::from_rgba8(0x3A, 0x2C, 0x1A, 0xFF) };
                let total_style = TextStyle { kind: FontKind::Annotation, px: TOTAL_PX, color };
                self.draw_text(pm, first.x as f32, y as f32 + TOTAL_PX * 0.35, &text, &total_style);
            }
        }
    }

    /// Draws the hover price-check popup: same parchment pill language as
    /// the row labels, but a single fixed-width block with a title line
    /// (item name) followed by one priced line per `popup.lines`, each with
    /// its currency icon composited the same way as `draw_frame`'s rows.
    /// `anchor` is the popup's top-left corner in surface-local pixels.
    /// Pixel size the popup pill will occupy, for placement and the
    /// move-away inside test. Must mirror draw_popup's layout math.
    pub fn popup_size(popup: &Popup) -> (i32, i32) {
        let title_h = POPUP_TITLE_PX + POPUP_ROW_GAP;
        let line_h = POPUP_LINE_PX + POPUP_ROW_GAP;
        let content_h = title_h + popup.lines.len() as f32 * line_h;
        let pill_h = content_h + POPUP_PAD * 2.0 - POPUP_ROW_GAP;
        (POPUP_WIDTH as i32, pill_h.ceil() as i32)
    }

    pub fn draw_popup(&self, pm: &mut Pixmap, popup: &Popup, anchor: (i32, i32)) {
        let (ax, ay) = anchor;
        let title_h = POPUP_TITLE_PX + POPUP_ROW_GAP;
        let line_h = POPUP_LINE_PX + POPUP_ROW_GAP;
        let content_h = title_h + popup.lines.len() as f32 * line_h;
        let pill_h = content_h + POPUP_PAD * 2.0 - POPUP_ROW_GAP;
        let pill_x = ax as f32;
        let pill_y = ay as f32;
        self.pill(pm, pill_x, pill_y, POPUP_WIDTH, pill_h, border_color(Tier::Decent));

        let title_color = Color::from_rgba8(0x3A, 0x2C, 0x1A, 0xFF);
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
