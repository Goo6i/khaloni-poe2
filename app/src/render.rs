use std::cell::RefCell;
use std::collections::HashMap;

use fontdue::{Font, FontSettings, Metrics};
use tiny_skia::{Color, Paint, PathBuilder, Pixmap, Rect as SkRect, Transform};

use crate::pricing::Tier;

pub struct Placed {
    pub x: i32,
    pub y: i32,
    pub label: String,
    pub tier: Tier,
}

// Keyed by (char, font size in integer tenths of a pixel) so the two fixed
// sizes used by draw_frame (LABEL_PX, TOTAL_PX) each get their own entries.
type GlyphKey = (char, u32);
type GlyphCache = RefCell<HashMap<GlyphKey, (Metrics, Vec<u8>)>>;

pub struct Renderer {
    font: Font,
    glyph_cache: GlyphCache,
}

const LABEL_PX: f32 = 16.0;
const TOTAL_PX: f32 = 18.0;

fn tier_color(t: Tier) -> Color {
    match t {
        Tier::Junk => Color::from_rgba8(0xB0, 0xB0, 0xB0, 0xFF),
        Tier::Decent => Color::from_rgba8(0xFF, 0xFF, 0xFF, 0xFF),
        Tier::Jackpot => Color::from_rgba8(0xFF, 0xA0, 0x36, 0xFF),
        Tier::Unknown => Color::from_rgba8(0x80, 0x80, 0x80, 0xFF),
    }
}

impl Renderer {
    pub fn new(font_bytes: &[u8]) -> anyhow::Result<Renderer> {
        let font = Font::from_bytes(font_bytes, FontSettings::default())
            .map_err(|e| anyhow::anyhow!("font load: {e}"))?;
        Ok(Renderer { font, glyph_cache: RefCell::new(HashMap::new()) })
    }

    fn glyph_key(ch: char, px: f32) -> GlyphKey {
        (ch, (px * 10.0).round() as u32)
    }

    fn text_width(&self, text: &str, px: f32) -> f32 {
        text.chars()
            .map(|c| self.font.metrics(c, px).advance_width)
            .sum()
    }

    fn draw_text(&self, pm: &mut Pixmap, x: f32, y_baseline: f32, text: &str, px: f32, color: Color) {
        let mut pen = x;
        let (cr, cg, cb) = (
            (color.red() * 255.0) as u16,
            (color.green() * 255.0) as u16,
            (color.blue() * 255.0) as u16,
        );
        let mut cache = self.glyph_cache.borrow_mut();
        for ch in text.chars() {
            let (metrics, bitmap) = cache
                .entry(Self::glyph_key(ch, px))
                .or_insert_with(|| self.font.rasterize(ch, px));
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
                let a = *cov as u16;
                // Premultiplied over-compositing of the glyph onto the pixmap.
                data[idx] = ((cr * a / 255) as u8).max(data[idx]);
                data[idx + 1] = ((cg * a / 255) as u8).max(data[idx + 1]);
                data[idx + 2] = ((cb * a / 255) as u8).max(data[idx + 2]);
                data[idx + 3] = (*cov).max(data[idx + 3]);
            }
            pen += metrics.advance_width;
        }
    }

    fn pill(&self, pm: &mut Pixmap, x: f32, y_top: f32, w: f32, h: f32) {
        let mut paint = Paint::default();
        paint.set_color(Color::from_rgba8(0x14, 0x12, 0x0e, 0xD8));
        paint.anti_alias = true;
        if let Some(rect) = SkRect::from_xywh(x, y_top, w, h) {
            let r = h / 2.0;
            let mut pb = PathBuilder::new();
            pb.move_to(rect.left() + r, rect.top());
            pb.line_to(rect.right() - r, rect.top());
            pb.quad_to(rect.right(), rect.top(), rect.right(), rect.top() + r);
            pb.quad_to(rect.right(), rect.bottom(), rect.right() - r, rect.bottom());
            pb.line_to(rect.left() + r, rect.bottom());
            pb.quad_to(rect.left(), rect.bottom(), rect.left(), rect.bottom() - r);
            pb.quad_to(rect.left(), rect.top(), rect.left() + r, rect.top());
            pb.close();
            if let Some(path) = pb.finish() {
                pm.fill_path(&path, &paint, tiny_skia::FillRule::Winding, Transform::identity(), None);
            }
        }
    }

    pub fn draw_frame(&self, pm: &mut Pixmap, labels: &[Placed], total: &str, stale: bool) {
        pm.fill(Color::TRANSPARENT);
        for p in labels {
            let tw = self.text_width(&p.label, LABEL_PX);
            let (pad_x, pill_h) = (8.0, LABEL_PX + 8.0);
            self.pill(pm, p.x as f32 - pad_x, p.y as f32 - pill_h / 2.0, tw + pad_x * 2.0, pill_h);
            self.draw_text(
                pm,
                p.x as f32,
                p.y as f32 + LABEL_PX * 0.35,
                &p.label,
                LABEL_PX,
                tier_color(p.tier),
            );
        }
        if !total.is_empty() {
            if let Some(first) = labels.iter().min_by_key(|p| p.y) {
                let text = if stale {
                    format!("{total}  [STALE]")
                } else {
                    total.to_string()
                };
                let tw = self.text_width(&text, TOTAL_PX);
                let y = (first.y - 24).max(14);
                self.pill(pm, first.x as f32 - 8.0, y as f32 - (TOTAL_PX + 8.0) / 2.0, tw + 16.0, TOTAL_PX + 8.0);
                let color = if stale {
                    Color::from_rgba8(0xE0, 0x50, 0x50, 0xFF)
                } else {
                    Color::from_rgba8(0xFF, 0xE0, 0x90, 0xFF)
                };
                self.draw_text(pm, first.x as f32, y as f32 + TOTAL_PX * 0.35, &text, TOTAL_PX, color);
            }
        }
    }
}
