//! Colours, type sizes and timing shared by the overlay renderer
//! (`render.rs`) and the settings window (`settings_style.rs`), so the two
//! read as one product.
//!
//! The palette, ornament metrics and timings follow PoE2 Oracle
//! (https://github.com/mttzzz/poe2-oracle, `crates/poe2-oracle/src/ui/theme.rs`
//! and `style.rs`, MIT OR Apache-2.0), ported with attribution. Colours
//! are `0xRRGGBB`.
//!
//! Used under the MIT licence; the notice is in `ornament.rs`.

use std::time::Duration;

// --- surfaces ------------------------------------------------------------
pub const BG_PANEL: u32 = 0x0e0e10;
pub const BG_TITLE: u32 = 0x060607;
pub const BG_NAMEPLATE: u32 = 0x17130d;
pub const BG_SIDEBAR: u32 = 0x0a0a0c;
pub const BG_CARD: u32 = 0x141417;
pub const BORDER_CARD: u32 = 0x25252b;
pub const BORDER_ROW: u32 = 0x1d1d22;
pub const BG_FIELD: u32 = 0x0b0b0d;
pub const BORDER_FIELD: u32 = 0x36363d;
pub const BG_MENU: u32 = 0x0c0c0e;
pub const BG_BUTTON_HOVER: u32 = 0x33261a;
/// The tooltip's black, behind a listing's hover card.
pub const BG_ITEM_CARD: u32 = 0x000000;
pub const PLATE_TOP: u32 = 0x3d2c16;
pub const PLATE_BOTTOM: u32 = 0x1f170c;
pub const TITLE_TOP: u32 = 0x1c160f;
pub const TITLE_BOTTOM: u32 = 0x09090a;
pub const KEY_TOP: u32 = 0x202026;

// --- gold and text -------------------------------------------------------
pub const BORDER_GOLD: u32 = 0x6b5022;
pub const GOLD: u32 = 0xd0913b;
pub const GOLD_LIGHT: u32 = 0xebc27a;
pub const TEXT: u32 = 0xdcdcdc;
pub const TEXT_DIM: u32 = 0x9d9da3;
pub const TEXT_MUTED: u32 = 0x66666c;
pub const TEXT_WARNING: u32 = 0xe0664f;
/// The game's colour for rolled values in item tooltips.
pub const TEXT_VALUE: u32 = 0x8888ff;
pub const TIER_TOP: u32 = 0xecc94b;
pub const PRICE_RISE: u32 = 0x4fc97f;
pub const PRICE_FALL: u32 = 0xe06c6c;
pub const BADGE_INK: u32 = 0x000000;

// --- names ---------------------------------------------------------------
pub const RARITY_NORMAL: u32 = 0xc8c8c8;
pub const RARITY_MAGIC: u32 = 0x8888ff;
pub const RARITY_RARE: u32 = 0xffff77;
pub const RARITY_UNIQUE: u32 = 0xaf6025;
pub const CURRENCY_NAME: u32 = 0xaa9e82;
pub const GEM_NAME: u32 = 0x1ba29b;
/// The nameplate's tint: this much of the name colour at the banner's top,
/// fading to none at its bottom; the rule under it takes `BANNER_EDGE`.
pub const BANNER_TINT: f32 = 0.16;
pub const BANNER_EDGE: f32 = 0.45;

// --- lens's own gutter colours (no Oracle equivalent) --------------------
/// Prefix and suffix badge letters: cool blue against warm amber, so the
/// two affix families split at a glance.
pub const AFFIX_PREFIX: u32 = 0x7fa8d6;
pub const AFFIX_SUFFIX: u32 = 0xd99a4a;
/// Seller status words in the listings table.
pub const STATUS_ONLINE: u32 = 0x4fc97f;
pub const STATUS_AFK: u32 = 0xed8936;

// --- reward-row pills over the game's own Runeshape rows -----------------
// Unchanged by the restyle: they sit on the game's art, not on a lens
// surface (spec, "Theme module").
pub const REWARD_PANEL: (u8, u8, u8, u8) = (0x1C, 0x16, 0x0F, 238);
pub const REWARD_INK: (u8, u8, u8) = (0xEA, 0xE0, 0xCB);
pub const REWARD_LINE: (u8, u8, u8) = (0x37, 0x2C, 0x1E);
pub const REWARD_BRONZE: (u8, u8, u8) = (0x6B, 0x56, 0x37);
pub const REWARD_GOLD: (u8, u8, u8) = (0xC9, 0xA2, 0x27);
pub const REWARD_BLUE: (u8, u8, u8) = (0x2E, 0x5A, 0x8A);
pub const REWARD_BLUE_LT: (u8, u8, u8) = (0x7F, 0xA8, 0xD6);
pub const REWARD_JUNK_LT: (u8, u8, u8) = (0xB7, 0xAB, 0x97);
pub const REWARD_RED: (u8, u8, u8) = (0x8B, 0x3A, 0x2E);

// --- motion and shape ----------------------------------------------------
/// Every hover and state change.
pub const TRANSITION: Duration = Duration::from_millis(120);
/// A panel appearing.
pub const APPEAR: Duration = Duration::from_millis(150);
/// How far an appearing panel rises into place, logical px.
pub const APPEAR_RISE: f32 = 6.0;
pub const CARD_RADIUS: f32 = 8.0;
pub const CONTROL_RADIUS: f32 = 4.0;

/// `over` laid on `base` at `amount` (0 is `base`, 1 is `over`).
pub fn blend(base: u32, over: u32, amount: f32) -> u32 {
    let channel = |shift: u32| {
        let from = ((base >> shift) & 0xff) as f32;
        let to = ((over >> shift) & 0xff) as f32;
        ((from + (to - from) * amount).round() as u32) << shift
    };
    channel(16) | channel(8) | channel(0)
}

/// CSS's `ease`, `cubic-bezier(0.25, 0.1, 0.25, 1)`: a quick start that
/// settles gently. Solved as browsers solve it: Newton's method on x(s),
/// bisection where it stalls. `t` outside 0..1 is clamped.
pub fn ease(t: f32) -> f32 {
    cubic_bezier((0.25, 0.1), (0.25, 1.0), t)
}

fn cubic_bezier((x1, y1): (f32, f32), (x2, y2): (f32, f32), x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    let (cx, cy) = (3.0 * x1, 3.0 * y1);
    let (bx, by) = (3.0 * (x2 - x1) - cx, 3.0 * (y2 - y1) - cy);
    let (ax, ay) = (1.0 - cx - bx, 1.0 - cy - by);
    let sample_x = |s: f32| ((ax * s + bx) * s + cx) * s;
    let slope_x = |s: f32| (3.0 * ax * s + 2.0 * bx) * s + cx;
    const EPSILON: f32 = 1e-5;
    let mut s = x;
    let mut solved = false;
    for _ in 0..8 {
        let error = sample_x(s) - x;
        if error.abs() < EPSILON {
            solved = true;
            break;
        }
        let slope = slope_x(s);
        if slope.abs() < 1e-6 {
            break;
        }
        s -= error / slope;
    }
    if !solved {
        let (mut low, mut high) = (0f32, 1f32);
        s = x;
        for _ in 0..32 {
            let error = sample_x(s) - x;
            if error.abs() < EPSILON {
                break;
            }
            if error > 0.0 {
                high = s;
            } else {
                low = s;
            }
            s = (low + high) / 2.0;
        }
    }
    (((ay * s + by) * s + cy) * s).clamp(0.0, 1.0)
}

pub fn color(c: u32) -> tiny_skia::Color {
    color_alpha(c, 0xff)
}

pub fn color_alpha(c: u32, alpha: u8) -> tiny_skia::Color {
    tiny_skia::Color::from_rgba8(((c >> 16) & 0xff) as u8, ((c >> 8) & 0xff) as u8, (c & 0xff) as u8, alpha)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blend_returns_its_ends_and_the_rounded_middle() {
        assert_eq!(blend(0x102030, 0x405060, 0.0), 0x102030);
        assert_eq!(blend(0x102030, 0x405060, 1.0), 0x405060);
        assert_eq!(blend(0x000000, 0xffffff, 0.5), 0x808080);
    }

    #[test]
    fn ease_is_the_css_ease_curve() {
        assert_eq!(ease(0.0), 0.0);
        assert_eq!(ease(1.0), 1.0);
        // cubic-bezier(0.25, 0.1, 0.25, 1) at x = 0.5 is 0.8024 (browsers agree to 3 places).
        assert!((ease(0.5) - 0.8024).abs() < 0.002, "{}", ease(0.5));
        let mut last = 0.0;
        for i in 1..=100 {
            let v = ease(i as f32 / 100.0);
            assert!(v >= last, "ease must not go back at {i}");
            last = v;
        }
        assert_eq!(ease(-1.0), 0.0);
        assert_eq!(ease(2.0), 1.0);
    }

    #[test]
    fn colours_convert_to_tiny_skia_with_full_or_given_alpha() {
        let c = color(GOLD);
        assert!((c.red() - 0xd0 as f32 / 255.0).abs() < 1e-6);
        assert!((c.alpha() - 1.0).abs() < 1e-6);
        let a = color_alpha(GOLD, 77);
        assert!((a.alpha() - 77.0 / 255.0).abs() < 1e-6);
    }
}
