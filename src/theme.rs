//! Brutalist look: square corners, thick ink borders, hard offset shadows,
//! bold monospace type and a flat yellow accent. Works in dark and light mode.

use std::sync::Arc;

use eframe::egui::{
    self, Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Rect, Response, Sense, Shadow,
    Stroke, StrokeKind, Style, TextStyle, Theme, Ui, Vec2, style::HandleShape, style::ScrollStyle, vec2,
};

pub const ACCENT: Color32 = Color32::from_rgb(255, 212, 0);
/// Text on the accent color, in both modes.
pub const ON_ACCENT: Color32 = Color32::from_rgb(17, 17, 17);
pub const ERROR: Color32 = Color32::from_rgb(255, 59, 48);
/// Snap guides on the paper.
pub const GUIDE: Color32 = Color32::from_rgb(255, 45, 120);
/// Offset of the hard shadows.
pub const SHADOW: f32 = 4.0;

pub struct Palette {
    /// Panels and buttons.
    pub panel: Color32,
    /// Slider rails, checkboxes, number fields.
    pub field: Color32,
    /// Text and borders.
    pub ink: Color32,
    pub weak: Color32,
    /// Background behind the paper and its grid lines.
    pub desk: Color32,
    pub grid: Color32,
}

pub fn palette(dark: bool) -> Palette {
    if dark {
        Palette {
            panel: Color32::from_gray(12),
            field: Color32::from_gray(34),
            ink: Color32::from_gray(240),
            weak: Color32::from_gray(150),
            desk: Color32::from_gray(38),
            grid: Color32::from_gray(50),
        }
    } else {
        Palette {
            panel: Color32::from_rgb(240, 238, 232),
            field: Color32::WHITE,
            ink: Color32::from_gray(17),
            weak: Color32::from_gray(90),
            desk: Color32::from_rgb(196, 193, 184),
            grid: Color32::from_rgb(181, 178, 169),
        }
    }
}

pub fn install(ctx: &egui::Context) {
    install_fonts(ctx);
    for theme in [Theme::Dark, Theme::Light] {
        ctx.style_mut_of(theme, |style| apply(style, theme == Theme::Dark));
    }
}

/// Font family for buttons and titles.
pub fn bold() -> FontFamily {
    FontFamily::Name("bold".into())
}

/// Bold monospace from the system (DejaVu or Liberation), falling back to egui's Hack.
fn install_fonts(ctx: &egui::Context) {
    let mut fonts = FontDefinitions::default();
    let mut family = Vec::new();
    let candidates = [
        "/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Bold.ttf",
        "/usr/share/fonts/TTF/DejaVuSansMono-Bold.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationMono-Bold.ttf",
    ];
    if let Some(bytes) = candidates.iter().find_map(|p| std::fs::read(p).ok()) {
        fonts.font_data.insert("bold-mono".into(), Arc::new(FontData::from_owned(bytes)));
        family.push("bold-mono".to_owned());
    }
    family.extend(fonts.families[&FontFamily::Monospace].iter().cloned());
    fonts.families.insert(bold(), family);
    ctx.set_fonts(fonts);
}

fn apply(style: &mut Style, dark: bool) {
    let p = palette(dark);
    let square = CornerRadius::ZERO;
    let border = Stroke::new(2.0, p.ink);

    style.text_styles = [
        (TextStyle::Small, FontId::monospace(11.0)),
        (TextStyle::Body, FontId::monospace(13.0)),
        (TextStyle::Monospace, FontId::monospace(13.0)),
        (TextStyle::Button, FontId::new(13.0, bold())),
        (TextStyle::Heading, FontId::new(18.0, bold())),
    ]
    .into();

    let s = &mut style.spacing;
    s.item_spacing = vec2(7.0, 8.0);
    s.button_padding = vec2(9.0, 5.0);
    s.interact_size = vec2(36.0, 26.0);
    s.slider_width = 84.0;
    s.slider_rail_height = 10.0;
    s.scroll = ScrollStyle::solid();

    let v = &mut style.visuals;
    v.dark_mode = dark;
    v.panel_fill = p.panel;
    v.window_fill = p.panel;
    v.extreme_bg_color = p.field;
    v.faint_bg_color = p.field;
    v.text_edit_bg_color = Some(p.field);
    v.weak_text_color = Some(p.weak);
    v.error_fg_color = ERROR;
    v.window_corner_radius = square;
    v.menu_corner_radius = square;
    v.window_stroke = border;
    v.window_shadow = Shadow { offset: [6, 6], blur: 0, spread: 0, color: p.ink };
    v.popup_shadow = Shadow { offset: [4, 4], blur: 0, spread: 0, color: p.ink };
    v.selection.bg_fill = ACCENT;
    v.selection.stroke = Stroke::new(2.0, ON_ACCENT);
    v.slider_trailing_fill = true;
    v.handle_shape = HandleShape::Rect { aspect_ratio: 0.6 };
    v.interact_cursor = Some(egui::CursorIcon::PointingHand);

    let w = &mut v.widgets;
    for (state, fill, text) in [
        (&mut w.noninteractive, p.panel, p.ink),
        (&mut w.inactive, p.panel, p.ink),
        (&mut w.hovered, ACCENT, ON_ACCENT),
        (&mut w.active, p.ink, p.panel),
        (&mut w.open, p.ink, p.panel),
    ] {
        state.corner_radius = square;
        state.expansion = 0.0;
        state.weak_bg_fill = fill;
        state.bg_fill = fill;
        state.bg_stroke = border;
        state.fg_stroke = Stroke::new(2.0, text);
    }
    // Checkbox boxes and slider rails stand out from the panel.
    w.inactive.bg_fill = p.field;
}

/// Big button with a hard shadow that sinks when pressed.
pub fn slab_button(ui: &mut Ui, text: &str, fill: Color32, text_color: Color32, enabled: bool, min: Vec2) -> Response {
    let font = FontId::new(17.0, bold());
    let galley = ui.painter().layout_no_wrap(text.to_owned(), font, Color32::PLACEHOLDER);
    let face_size = (galley.size() + vec2(28.0, 14.0)).max(min);
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let (rect, response) = ui.allocate_exact_size(face_size + Vec2::splat(SHADOW), sense);

    let ink = ui.visuals().widgets.noninteractive.fg_stroke.color;
    let face = Rect::from_min_size(rect.min, face_size);
    let pressed = enabled && response.is_pointer_button_down_on();
    let face = if pressed { face.translate(Vec2::splat(SHADOW)) } else { face };
    let painter = ui.painter();
    if !pressed {
        painter.rect_filled(face.translate(Vec2::splat(SHADOW)), 0.0, ink);
    }
    // Hover: plain buttons turn yellow, the yellow one turns ink.
    let (fill, text_color) = if !enabled {
        (fill.gamma_multiply(0.45), text_color.gamma_multiply(0.6))
    } else if response.hovered() {
        if fill == ACCENT { (ink, ACCENT) } else { (ACCENT, ON_ACCENT) }
    } else {
        (fill, text_color)
    };
    painter.rect(face, 0.0, fill, Stroke::new(2.0, ink), StrokeKind::Inside);
    painter.galley(face.center() - galley.size() / 2.0, galley, text_color);
    if enabled { response.on_hover_cursor(egui::CursorIcon::PointingHand) } else { response }
}
