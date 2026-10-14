//! The settings window's game-styled look: the theme's tokens as egui
//! visuals and fonts, the double gold frame painted over the viewport, and
//! the controls Oracle's settings use (title bar, sidebar items, section
//! heading with its rule, cards and rows, switch, segmented choice, stepper,
//! keycap, recorder, buttons), drawn with the egui painter so the window
//! reads as the same product as the overlay panels. Behaviour is egui's:
//! every control wraps an egui response.

use std::ops::RangeInclusive;

use eframe::egui::{self, Color32, Pos2, Rect, Response, Sense, Stroke, Ui, Vec2};

use crate::ornament::{self, DeviceRect, FrameMetrics, Scale, FRAME_CLEAR, RULE_DIAMOND};
use crate::theme::{self, blend};

const FONTIN_REGULAR: &[u8] = include_bytes!("../assets/fonts/Fontin-Regular.ttf");
const FONTIN_SMALLCAPS: &[u8] = include_bytes!("../assets/fonts/Fontin-SmallCaps.ttf");

pub const SIDEBAR_W: f32 = 190.0;
pub const TITLE_H: f32 = 40.0;
const ROW_H: f32 = 30.0;

pub fn color32(c: u32) -> Color32 {
    Color32::from_rgb(((c >> 16) & 0xff) as u8, ((c >> 8) & 0xff) as u8, (c & 0xff) as u8)
}

fn small_caps() -> egui::FontFamily {
    egui::FontFamily::Name("FontinSC".into())
}

pub fn fonts() -> egui::FontDefinitions {
    let mut f = egui::FontDefinitions::default();
    f.font_data.insert("Fontin".into(), std::sync::Arc::new(egui::FontData::from_static(FONTIN_REGULAR)));
    f.font_data.insert("Fontin SmallCaps".into(), std::sync::Arc::new(egui::FontData::from_static(FONTIN_SMALLCAPS)));
    f.families.entry(egui::FontFamily::Proportional).or_default().insert(0, "Fontin".into());
    f.families.insert(small_caps(), vec!["Fontin SmallCaps".into(), "Fontin".into()]);
    f
}

pub fn visuals() -> egui::Visuals {
    let mut v = egui::Visuals::dark();
    v.panel_fill = color32(theme::BG_PANEL);
    v.window_fill = color32(theme::BG_MENU);
    v.extreme_bg_color = color32(theme::BG_FIELD);
    v.faint_bg_color = color32(theme::BG_CARD);
    v.override_text_color = Some(color32(theme::TEXT));
    v.hyperlink_color = color32(theme::GOLD_LIGHT);
    v.selection.bg_fill = color32(blend(theme::BG_FIELD, theme::GOLD, 0.3));
    v.selection.stroke = Stroke::new(1.0, color32(theme::GOLD));
    let field = |w: &mut egui::style::WidgetVisuals, bg: u32, edge: u32, text: u32| {
        w.bg_fill = color32(bg);
        w.weak_bg_fill = color32(bg);
        w.bg_stroke = Stroke::new(1.0, color32(edge));
        w.fg_stroke = Stroke::new(1.0, color32(text));
        w.corner_radius = egui::CornerRadius::same(theme::CONTROL_RADIUS as u8);
    };
    field(&mut v.widgets.noninteractive, theme::BG_PANEL, theme::BORDER_ROW, theme::TEXT_DIM);
    field(&mut v.widgets.inactive, theme::BG_FIELD, theme::BORDER_FIELD, theme::TEXT);
    field(&mut v.widgets.hovered, theme::BG_BUTTON_HOVER, blend(theme::BORDER_FIELD, theme::GOLD, 0.6), theme::GOLD_LIGHT);
    field(&mut v.widgets.active, theme::BG_BUTTON_HOVER, theme::GOLD, theme::GOLD_LIGHT);
    field(&mut v.widgets.open, theme::BG_MENU, theme::BORDER_GOLD, theme::TEXT);
    v.window_stroke = Stroke::new(1.0, color32(theme::BORDER_GOLD));
    v
}

/// Fonts, visuals and spacing, installed once per context.
pub fn install(ctx: &egui::Context) {
    let id = egui::Id::new("settings-style-installed");
    if ctx.data(|d| d.get_temp::<bool>(id)).unwrap_or(false) {
        return;
    }
    ctx.set_fonts(fonts());
    ctx.set_visuals(visuals());
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = Vec2::new(10.0, 8.0);
        s.spacing.button_padding = Vec2::new(12.0, 5.0);
        s.text_styles.insert(egui::TextStyle::Heading, egui::FontId::new(28.0, small_caps()));
        s.text_styles.insert(egui::TextStyle::Body, egui::FontId::new(15.0, egui::FontFamily::Proportional));
        s.text_styles.insert(egui::TextStyle::Small, egui::FontId::new(13.0, egui::FontFamily::Proportional));
        s.text_styles.insert(egui::TextStyle::Button, egui::FontId::new(14.0, egui::FontFamily::Proportional));
    });
    // Hairlines land on device pixels only without feathering.
    ctx.tessellation_options_mut(|o| o.feathering = false);
    ctx.data_mut(|d| d.insert_temp(id, true));
}

/// The frame's parts for `rect` (points) at `ppp` device pixels per point.
pub fn frame_parts(rect: Rect, ppp: f32) -> ornament::FrameParts {
    let scale = Scale { device: ppp, unit: 1.0 };
    let dr = DeviceRect::new(scale.device(rect.left()), scale.device(rect.top()), scale.device(rect.right()), scale.device(rect.bottom()));
    ornament::frame(dr, FrameMetrics::new(scale))
}

fn points(r: DeviceRect, ppp: f32) -> Rect {
    Rect::from_min_max(Pos2::new(r.left as f32 / ppp, r.top as f32 / ppp), Pos2::new(r.right as f32 / ppp, r.bottom as f32 / ppp))
}

/// Rasterised sprites uploaded once and kept in the context's temp data,
/// keyed by the sprite's `Debug` string, so a frame re-uploads nothing.
#[derive(Clone, Default)]
struct SpriteTextures(std::collections::HashMap<String, egui::TextureHandle>);

fn sprite_texture(ctx: &egui::Context, sprite: ornament::Sprite) -> egui::TextureHandle {
    let name = format!("ornament-{sprite:?}");
    let id = egui::Id::new("settings-style-sprites");
    if let Some(tex) = ctx.data(|d| d.get_temp::<SpriteTextures>(id)).and_then(|c| c.0.get(&name).cloned()) {
        return tex;
    }
    let img = ornament::rasterize(&sprite);
    let mut pixels = Vec::with_capacity((img.width() * img.height()) as usize);
    for p in img.pixels() {
        let image::Rgba([b, g, r, a]) = *p;
        pixels.push(Color32::from_rgba_unmultiplied(r, g, b, a));
    }
    let color = egui::ColorImage::new([img.width() as usize, img.height() as usize], pixels);
    let tex = ctx.load_texture(name.clone(), color, egui::TextureOptions::NEAREST);
    ctx.data_mut(|d| d.get_temp_mut_or_default::<SpriteTextures>(id).0.insert(name, tex.clone()));
    tex
}

/// Paints a `Placed` sprite through `painter` at `ppp`.
fn paint_sprite(ctx: &egui::Context, painter: &egui::Painter, placed: &ornament::Placed, ppp: f32) {
    let tex = sprite_texture(ctx, placed.sprite);
    let full = points(placed.rect, ppp);
    let shown = points(placed.shown, ppp);
    let uv = Rect::from_min_max(
        Pos2::new((shown.left() - full.left()) / full.width(), (shown.top() - full.top()) / full.height()),
        Pos2::new((shown.right() - full.left()) / full.width(), (shown.bottom() - full.top()) / full.height()),
    );
    painter.image(tex.id(), shown, uv, Color32::WHITE);
}

/// The double gold frame over the whole viewport, on the foreground layer.
pub fn frame(ctx: &egui::Context) {
    let rect = ctx.viewport_rect();
    let ppp = ctx.pixels_per_point();
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("game-frame")));
    let parts = frame_parts(rect, ppp);
    for line in &parts.lines {
        if line.rect.is_empty() { continue; }
        let c = color32(line.color).gamma_multiply(1.0);
        let c = Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), line.alpha);
        painter.rect_filled(points(line.rect, ppp), 0.0, c);
    }
    for corner in &parts.corners {
        paint_sprite(ctx, &painter, corner, ppp);
    }
}

pub struct TitleBar {
    pub close_clicked: bool,
}

/// The title bar: the gradient, a gold diamond, the product name in small
/// caps, a dot, `title`, and × at the right. The empty part drags the window.
pub fn title_bar(ui: &mut Ui, title: &str) -> TitleBar {
    let full = ui.available_rect_before_wrap();
    let rect = Rect::from_min_size(full.min, Vec2::new(full.width(), TITLE_H));
    let (rect, resp) = ui.allocate_exact_size(rect.size(), Sense::click_and_drag());
    let painter = ui.painter();
    // Gradient as two bands under the frame's keep-out.
    let inner = rect.shrink2(Vec2::new(FRAME_CLEAR, 0.0)).with_min_y(rect.top() + FRAME_CLEAR);
    let half = Rect::from_min_max(inner.min, Pos2::new(inner.right(), inner.center().y));
    painter.rect_filled(half, 0.0, color32(theme::TITLE_TOP));
    painter.rect_filled(Rect::from_min_max(half.left_bottom(), inner.max), 0.0, color32(theme::TITLE_BOTTOM));
    painter.rect_filled(Rect::from_min_max(Pos2::new(inner.left(), inner.bottom() - 1.0), inner.max), 0.0, color32(theme::BORDER_GOLD));
    let ppp = ui.ctx().pixels_per_point();
    let scale = Scale { device: ppp, unit: 1.0 };
    let dx = inner.left() + 16.0;
    let dy = inner.center().y;
    let drect = DeviceRect::new(scale.device(dx - 4.0), scale.device(dy - 4.0), scale.device(dx + 4.0), scale.device(dy + 4.0));
    paint_sprite(ui.ctx(), painter, &ornament::diamond(drect, 8.0, scale, theme::GOLD, false), ppp);
    let name_font = egui::FontId::new(18.0, small_caps());
    painter.text(Pos2::new(dx + 14.0, dy), egui::Align2::LEFT_CENTER, "khaloni-poe2", name_font.clone(), color32(theme::GOLD_LIGHT));
    let name_w = painter.layout_no_wrap("khaloni-poe2".into(), name_font.clone(), Color32::WHITE).size().x;
    painter.text(Pos2::new(dx + 14.0 + name_w + 10.0, dy), egui::Align2::LEFT_CENTER, "·", name_font.clone(), color32(theme::TEXT_MUTED));
    painter.text(Pos2::new(dx + 14.0 + name_w + 24.0, dy), egui::Align2::LEFT_CENTER, title, name_font, color32(theme::TEXT));
    let close = Rect::from_center_size(Pos2::new(inner.right() - 20.0, dy), Vec2::splat(24.0));
    let close_resp = ui.interact(close, ui.id().with("title-close"), Sense::click());
    if close_resp.hovered() {
        painter.rect_filled(close, theme::CONTROL_RADIUS, color32(theme::BG_BUTTON_HOVER));
    }
    // Fontin has no × glyph, so the cross is two strokes.
    let ink = Stroke::new(1.5, color32(if close_resp.hovered() { theme::GOLD_LIGHT } else { theme::TEXT_DIM }));
    let arm = Vec2::splat(5.0);
    painter.line_segment([close.center() - arm, close.center() + arm], ink);
    painter.line_segment([close.center() + Vec2::new(-arm.x, arm.y), close.center() + Vec2::new(arm.x, -arm.y)], ink);
    if resp.drag_started() && !close_resp.hovered() {
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }
    TitleBar { close_clicked: close_resp.clicked() }
}

pub fn sidebar_item(ui: &mut Ui, label: &str, selected: bool) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 36.0), Sense::click());
    let painter = ui.painter();
    if selected {
        painter.rect_filled(rect, theme::CONTROL_RADIUS, color32(theme::BG_BUTTON_HOVER));
        painter.rect_filled(Rect::from_min_size(rect.min, Vec2::new(2.0, rect.height())), 0.0, color32(theme::GOLD));
    } else if resp.hovered() {
        painter.rect_filled(rect, theme::CONTROL_RADIUS, color32(blend(theme::BG_SIDEBAR, theme::BG_BUTTON_HOVER, 0.5)));
    }
    let ink = if selected || resp.hovered() { theme::GOLD_LIGHT } else { theme::TEXT_DIM };
    painter.text(Pos2::new(rect.left() + 16.0, rect.center().y), egui::Align2::LEFT_CENTER, label, egui::FontId::new(15.0, small_caps()), color32(ink));
    resp
}

pub fn sidebar_footer(ui: &mut Ui, name: &str, version: &str, status: Option<&str>) {
    ui.add_space(8.0);
    ui.label(egui::RichText::new(name).font(egui::FontId::new(14.0, small_caps())).color(color32(theme::GOLD)));
    ui.label(egui::RichText::new(format!("version {version}")).small().color(color32(theme::TEXT_MUTED)));
    if let Some(s) = status {
        ui.label(egui::RichText::new(s).small().color(color32(theme::TEXT_DIM)));
    }
}

/// A rule with a diamond at its centre, the lines fading toward the ends.
pub fn ornament_rule(ui: &mut Ui, color: u32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), RULE_DIAMOND), Sense::hover());
    let ppp = ui.ctx().pixels_per_point();
    let scale = Scale { device: ppp, unit: 1.0 };
    let dr = DeviceRect::new(scale.device(rect.left()), scale.device(rect.top()), scale.device(rect.right()), scale.device(rect.bottom()));
    let painter = ui.painter().clone();
    for part in ornament::rule(dr, scale, color, theme::GOLD).iter().flatten() {
        paint_sprite(ui.ctx(), &painter, part, ppp);
    }
}

pub fn section_heading(ui: &mut Ui, title: &str, description: &str) {
    ui.add_space(16.0);
    ui.label(egui::RichText::new(title).font(egui::FontId::new(28.0, small_caps())).color(color32(theme::GOLD_LIGHT)));
    ui.label(egui::RichText::new(description).color(color32(theme::TEXT_DIM)));
    ui.add_space(6.0);
    ornament_rule(ui, theme::BORDER_GOLD);
    ui.add_space(10.0);
}

/// A group heading inside a section: diamond, capitals in gold, fading rule.
pub fn group_heading(ui: &mut Ui, title: &str) {
    ui.add_space(8.0);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 18.0), Sense::hover());
    let ppp = ui.ctx().pixels_per_point();
    let scale = Scale { device: ppp, unit: 1.0 };
    let painter = ui.painter().clone();
    let cy = rect.center().y;
    let dr = DeviceRect::new(scale.device(rect.left()), scale.device(cy - 3.0), scale.device(rect.left() + 6.0), scale.device(cy + 3.0));
    paint_sprite(ui.ctx(), &painter, &ornament::diamond(dr, 6.0, scale, theme::GOLD, true), ppp);
    let font = egui::FontId::new(12.5, small_caps());
    let text = title.to_uppercase();
    let tw = painter.layout_no_wrap(text.clone(), font.clone(), Color32::WHITE).size().x;
    painter.text(Pos2::new(rect.left() + 13.0, cy), egui::Align2::LEFT_CENTER, text, font, color32(theme::GOLD));
    let lr = DeviceRect::new(scale.device(rect.left() + 13.0 + tw + 7.0), scale.device(cy - 0.5), scale.device(rect.right()), scale.device(cy + 0.5));
    if let Some(line) = ornament::fading_line(lr, scale, theme::BORDER_GOLD, false) {
        paint_sprite(ui.ctx(), &painter, &line, ppp);
    }
    ui.add_space(4.0);
}

/// A card: BG_CARD fill, BORDER_CARD edge, CARD_RADIUS corners; rows inside.
pub fn card(ui: &mut Ui, id: &str, add_contents: impl FnOnce(&mut Ui)) {
    egui::Frame::NONE
        .fill(color32(theme::BG_CARD))
        .stroke(Stroke::new(1.0, color32(theme::BORDER_CARD)))
        .corner_radius(theme::CARD_RADIUS)
        .inner_margin(egui::Margin::symmetric(16, 4))
        .show(ui, |ui| {
            ui.push_id(id, |ui| {
                ui.set_min_width(ui.available_width());
                add_contents(ui);
            });
        });
    ui.add_space(12.0);
}

/// A card row: label and description at the left, the control at the right,
/// a BORDER_ROW hairline under it. The last row of a card passes
/// `last = true` through `row_last` to skip the hairline.
pub fn row(ui: &mut Ui, label: &str, description: Option<&str>, control: impl FnOnce(&mut Ui)) {
    row_inner(ui, label, description, false, control);
}

pub fn row_last(ui: &mut Ui, label: &str, description: Option<&str>, control: impl FnOnce(&mut Ui)) {
    row_inner(ui, label, description, true, control);
}

fn row_inner(ui: &mut Ui, label: &str, description: Option<&str>, last: bool, control: impl FnOnce(&mut Ui)) {
    ui.add_space(8.0);
    ui.horizontal(|ui| {
        // A row with no label and no description is all control: no left
        // column, so the control gets the whole width.
        if !label.is_empty() || description.is_some() {
            ui.vertical(|ui| {
                ui.set_width(ui.available_width() * 0.6);
                ui.label(egui::RichText::new(label).size(16.0).color(color32(theme::TEXT)));
                if let Some(d) = description {
                    ui.label(egui::RichText::new(d).small().color(color32(theme::TEXT_DIM)));
                }
            });
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| control(ui));
    });
    ui.add_space(8.0);
    if !last {
        // One device pixel on a device row, at any scale.
        let r = ui.available_rect_before_wrap();
        let ppp = ui.ctx().pixels_per_point();
        let y = (r.top() * ppp).round() / ppp;
        ui.painter().rect_filled(Rect::from_min_size(Pos2::new(r.left(), y), Vec2::new(r.width(), 1.0 / ppp)), 0.0, color32(theme::BORDER_ROW));
        ui.add_space(1.0);
    }
}

/// A switch: 34×18 track, gold when on, knob sliding over TRANSITION.
pub fn switch(ui: &mut Ui, on: &mut bool) -> Response {
    let (rect, mut resp) = ui.allocate_exact_size(Vec2::new(34.0, 18.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_with_time(resp.id, *on, theme::TRANSITION.as_secs_f32());
    let painter = ui.painter();
    painter.rect(rect, 9.0, color32(blend(theme::BG_FIELD, blend(theme::BG_FIELD, theme::GOLD, 0.3), t)),
        Stroke::new(1.0, color32(blend(theme::BORDER_FIELD, theme::GOLD, t))), egui::StrokeKind::Inside);
    let knob = Pos2::new(rect.left() + 8.0 + 16.0 * t, rect.center().y);
    painter.circle_filled(knob, 6.0, color32(blend(theme::TEXT_DIM, theme::GOLD_LIGHT, t)));
    resp
}

/// Choices side by side in one field; the picked one lit gold. Returns
/// whether the value changed.
pub fn segmented<T: Copy + PartialEq>(ui: &mut Ui, id: &str, options: &[(T, &str)], value: &mut T) -> bool {
    let mut changed = false;
    egui::Frame::NONE
        .fill(color32(theme::BG_FIELD))
        .stroke(Stroke::new(1.0, color32(theme::BORDER_FIELD)))
        .corner_radius(theme::CONTROL_RADIUS)
        .inner_margin(egui::Margin::same(2))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for (i, (v, label)) in options.iter().enumerate() {
                    let lit = *v == *value;
                    let galley = ui.painter().layout_no_wrap(label.to_string(), egui::FontId::new(13.0, egui::FontFamily::Proportional), Color32::WHITE);
                    let (rect, resp) = ui.allocate_exact_size(Vec2::new(galley.size().x + 24.0, ROW_H - 6.0), Sense::click());
                    let t = ui.ctx().animate_bool_with_time(ui.id().with((id, i)), lit, theme::TRANSITION.as_secs_f32());
                    let h = ui.ctx().animate_bool_with_time(ui.id().with((id, i, "h")), resp.hovered(), theme::TRANSITION.as_secs_f32());
                    let painter = ui.painter();
                    let fill = Color32::from_rgba_unmultiplied(0xd0, 0x91, 0x3b, (0.16 * t * 255.0) as u8);
                    let edge = Color32::from_rgba_unmultiplied(0xd0, 0x91, 0x3b, (0.6 * t * 255.0) as u8);
                    painter.rect(rect, theme::CONTROL_RADIUS - 1.0, fill, Stroke::new(1.0, edge), egui::StrokeKind::Inside);
                    let ink = blend(blend(theme::TEXT_DIM, theme::TEXT, h), theme::GOLD_LIGHT, t);
                    painter.galley(Pos2::new(rect.left() + 12.0, rect.center().y - galley.size().y / 2.0), galley, color32(ink));
                    if resp.clicked() && !lit {
                        *value = *v;
                        changed = true;
                    }
                }
            });
        });
    changed
}

/// − value + : an outlined button each side of egui's own DragValue, so
/// the number can still be typed or dragged.
pub fn stepper(ui: &mut Ui, value: &mut f64, step: f64, range: RangeInclusive<f64>, suffix: &str) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        // The row slot is right to left: + first, so it reads "− value +".
        if button(ui, "+").clicked() {
            *value = (*value + step).clamp(*range.start(), *range.end());
            changed = true;
        }
        let drag = ui.add(egui::DragValue::new(value).speed(step).range(range.clone()).suffix(suffix));
        changed |= drag.changed();
        if button(ui, "−").clicked() {
            *value = (*value - step).clamp(*range.start(), *range.end());
            changed = true;
        }
    });
    changed
}

/// A keycap: KEY_TOP fading to BG_FIELD, BORDER_FIELD edge, the key in small caps.
pub fn keycap(ui: &mut Ui, label: &str) {
    let font = egui::FontId::new(13.0, small_caps());
    let galley = ui.painter().layout_no_wrap(label.to_string(), font, Color32::WHITE);
    let (rect, _) = ui.allocate_exact_size(Vec2::new(galley.size().x + 16.0, 24.0), Sense::hover());
    let painter = ui.painter();
    let top = Rect::from_min_max(rect.min, Pos2::new(rect.right(), rect.center().y));
    painter.rect_filled(rect, theme::CONTROL_RADIUS, color32(theme::BG_FIELD));
    painter.rect_filled(top, theme::CONTROL_RADIUS, color32(theme::KEY_TOP));
    painter.rect_stroke(rect, theme::CONTROL_RADIUS, Stroke::new(1.0, color32(theme::BORDER_FIELD)), egui::StrokeKind::Inside);
    painter.galley(Pos2::new(rect.left() + 8.0, rect.center().y - galley.size().y / 2.0), galley, color32(theme::TEXT));
}

/// The hotkey recorder: a keycap-styled button showing the binding, gold
/// while armed ("press a key…").
pub fn recorder(ui: &mut Ui, current: &str, armed: bool) -> Response {
    let label = if armed { "press a key…" } else if current.is_empty() { "unbound" } else { current };
    let font = egui::FontId::new(13.0, small_caps());
    let galley = ui.painter().layout_no_wrap(label.to_string(), font, Color32::WHITE);
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(galley.size().x.max(60.0) + 16.0, 24.0), Sense::click());
    let painter = ui.painter();
    let edge = if armed { theme::GOLD } else if resp.hovered() { blend(theme::BORDER_FIELD, theme::GOLD, 0.6) } else { theme::BORDER_FIELD };
    painter.rect_filled(rect, theme::CONTROL_RADIUS, color32(theme::BG_FIELD));
    painter.rect_filled(Rect::from_min_max(rect.min, Pos2::new(rect.right(), rect.center().y)), theme::CONTROL_RADIUS, color32(theme::KEY_TOP));
    painter.rect_stroke(rect, theme::CONTROL_RADIUS, Stroke::new(1.0, color32(edge)), egui::StrokeKind::Inside);
    painter.galley(Pos2::new(rect.left() + 8.0, rect.center().y - galley.size().y / 2.0), galley, color32(if armed { theme::GOLD_LIGHT } else { theme::TEXT }));
    resp
}

/// An outlined button.
pub fn button(ui: &mut Ui, label: &str) -> Response {
    ui.add(egui::Button::new(egui::RichText::new(label).color(color32(theme::TEXT))).fill(Color32::TRANSPARENT).stroke(Stroke::new(1.0, color32(theme::BORDER_FIELD))))
}

/// The bronze plate button, for the one primary action of a section.
pub fn plate_button(ui: &mut Ui, label: &str) -> Response {
    let text = egui::RichText::new(label).font(egui::FontId::new(15.0, small_caps())).color(color32(theme::GOLD_LIGHT));
    ui.add(egui::Button::new(text).fill(color32(theme::PLATE_TOP)).stroke(Stroke::new(1.0, color32(theme::BORDER_GOLD))))
}

pub fn text_field(ui: &mut Ui, text: &mut String) -> Response {
    ui.add(egui::TextEdit::singleline(text).desired_width(220.0))
}

pub fn warning(ui: &mut Ui, text: &str) {
    ui.label(egui::RichText::new(text).color(color32(theme::TEXT_WARNING)));
}

pub fn note(ui: &mut Ui, text: &str) {
    ui.label(egui::RichText::new(text).small().color(color32(theme::TEXT_DIM)));
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color32_carries_the_channels_over_opaque() {
        let c = color32(0x6b5022);
        assert_eq!((c.r(), c.g(), c.b(), c.a()), (0x6b, 0x50, 0x22, 255));
    }

    #[test]
    fn the_visuals_are_dark_with_the_theme_fills() {
        let v = visuals();
        assert!(v.dark_mode);
        assert_eq!(v.panel_fill, color32(crate::theme::BG_PANEL));
        assert_eq!(v.widgets.inactive.bg_fill, color32(crate::theme::BG_FIELD));
        assert_eq!(v.selection.bg_fill, color32(crate::theme::blend(crate::theme::BG_FIELD, crate::theme::GOLD, 0.3)));
    }

    #[test]
    fn the_fonts_lead_with_fontin_and_name_the_small_caps_family() {
        let f = fonts();
        assert_eq!(f.families[&egui::FontFamily::Proportional][0], "Fontin");
        assert_eq!(f.families[&egui::FontFamily::Name("FontinSC".into())][0], "Fontin SmallCaps");
        assert!(f.font_data.contains_key("Fontin") && f.font_data.contains_key("Fontin SmallCaps"));
    }

    #[test]
    fn frame_rects_follow_the_ornament_module_at_the_windows_scale() {
        let parts = frame_parts(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(300.0, 200.0)), 1.5);
        // Outer line is one device px wide at the very edge: 0..1 device px = 0..(1/1.5) points.
        let left = parts.lines[2];
        assert_eq!(left.rect.left, 0);
        assert_eq!(left.rect.right, 1);
    }
}
