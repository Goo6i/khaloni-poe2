//! Popup placement and move-away dismissal, pure geometry. The popup
//! opens offset from the cursor so it never covers the hovered item, and
//! closes when the cursor walks away from where the check happened,
//! unless it walked INTO the popup (reading listings, clicking mods).
//! Pattern taken from Awakened PoE Trade's WidgetAreaTracker, adapted
//! from screen-half docking to true cursor anchoring.

use crate::config::Rect;

/// Offset from the cursor to the popup's top-left, so the pill sits
/// beside the pointer instead of under it.
const OFFSET: i32 = 24;
/// How far (logical px) the cursor may drift from the check position
/// before the popup closes. APT uses 2.5em (~40px at 16px font); the
/// game runs at 1.5x scale on a 4K panel, so 90 logical px lands in the
/// same perceptual range.
const CLOSE_DISTANCE: f64 = 90.0;
/// The popup rect is inflated by this much for the inside test, so
/// grazing its edge on the way in does not count as "outside".
const GRACE: i32 = 12;

/// Anchor for a popup of `size` opened with the cursor at `cursor`,
/// kept fully inside `game`. Prefers below-right of the cursor; flips
/// to the left when the right edge would overflow, and above when the
/// bottom would.
pub fn place(cursor: (i32, i32), size: (i32, i32), game: Rect) -> (i32, i32) {
    let (cw, ch) = size;
    let right = game.x + game.w as i32;
    let bottom = game.y + game.h as i32;
    let mut x = cursor.0 + OFFSET;
    if x + cw > right {
        x = cursor.0 - OFFSET - cw;
    }
    let mut y = cursor.1 + OFFSET;
    if y + ch > bottom {
        y = cursor.1 - OFFSET - ch;
    }
    (x.clamp(game.x, (right - cw).max(game.x)), y.clamp(game.y, (bottom - ch).max(game.y)))
}

/// True when the popup should close: the cursor has moved more than
/// CLOSE_DISTANCE from where the check fired AND is not inside the
/// (slightly inflated) popup rect.
pub fn should_dismiss(origin: (i32, i32), cursor: (i32, i32), popup: Rect) -> bool {
    let dx = f64::from(cursor.0 - origin.0);
    let dy = f64::from(cursor.1 - origin.1);
    if dx.hypot(dy) <= CLOSE_DISTANCE {
        return false;
    }
    let inside = cursor.0 >= popup.x - GRACE
        && cursor.0 <= popup.x + popup.w as i32 + GRACE
        && cursor.1 >= popup.y - GRACE
        && cursor.1 <= popup.y + popup.h as i32 + GRACE;
    !inside
}

#[cfg(test)]
mod tests {
    use super::*;

    const GAME: Rect = Rect { x: 2560, y: 0, w: 2560, h: 1440 };

    #[test]
    fn places_below_right_of_the_cursor() {
        assert_eq!(place((3000, 500), (300, 200), GAME), (3024, 524));
    }

    #[test]
    fn flips_left_when_the_right_edge_would_overflow() {
        // Cursor near the right edge: 5000+24+300 > 5120.
        assert_eq!(place((5000, 500), (300, 200), GAME), (5000 - 24 - 300, 524));
    }

    #[test]
    fn flips_up_when_the_bottom_would_overflow() {
        assert_eq!(place((3000, 1400), (300, 200), GAME), (3024, 1400 - 24 - 200));
    }

    #[test]
    fn clamps_inside_the_game_rect_in_the_corner_case() {
        // Top-left corner: the flip would leave the rect; clamp holds it in.
        let (x, y) = place((2560, 0), (300, 200), GAME);
        assert!(x >= GAME.x && y >= GAME.y);
        assert!(x + 300 <= GAME.x + GAME.w as i32 && y + 200 <= GAME.y + GAME.h as i32);
    }

    #[test]
    fn near_moves_do_not_dismiss() {
        let popup = Rect { x: 3024, y: 524, w: 300, h: 200 };
        assert!(!should_dismiss((3000, 500), (3050, 560), popup));
    }

    #[test]
    fn far_moves_dismiss() {
        let popup = Rect { x: 3024, y: 524, w: 300, h: 200 };
        assert!(should_dismiss((3000, 500), (3600, 1100), popup));
    }

    #[test]
    fn far_moves_into_the_popup_do_not_dismiss() {
        let popup = Rect { x: 3024, y: 524, w: 300, h: 200 };
        // Deep inside the popup, well past CLOSE_DISTANCE from the origin.
        assert!(!should_dismiss((3000, 500), (3200, 700), popup));
        // Grazing just outside the edge stays alive too (GRACE).
        assert!(!should_dismiss((3000, 500), (3330, 700), popup));
    }
}
