//! Window to place, scale and print an image on the paper strip.
//!
//! The window is the paper: drag the image to move it, drag a corner (or Ctrl + wheel) to scale
//! it and drag the bottom edge of the paper to cut it. Every control is in the bottom bar.

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use eframe::egui::{
    self, Align, Align2, Color32, ColorImage, CursorIcon, FontId, Layout, Painter, Pos2, Rect, RichText, Sense, Shape,
    Stroke, TextureFilter, TextureHandle, TextureOptions, Vec2, pos2, vec2,
};
use image::{GrayImage, imageops};

use crate::imaging::{self, Placement, Settings};
use crate::printer::{self, Config, DOTS_PER_MM, PAPER_WIDTH, PAPER_WIDTH_MM};
use crate::theme::{self, ACCENT, ERROR, GUIDE, ON_ACCENT, Palette};

/// Thermal paper color; the white pixels of the print are tinted with it.
const PAPER: Color32 = Color32::from_rgb(250, 250, 246);
/// Blank paper drawn above and below the print. Visual only: the real paper feed is unknown.
const LEAD_MM: f32 = 3.0;
/// Longest side of the faint copy of the image drawn where it does not print.
const GHOST_SIDE: u32 = 1024;
/// Screen distance at which edges snap and corners are grabbed.
const SNAP: f32 = 7.0;
const HANDLE: f32 = 11.0;
/// Room around the paper for the ruler and the dimensions.
const MARGIN_LEFT: f32 = 52.0;
const MARGIN_RIGHT: f32 = 92.0;
const MARGIN_TOP: f32 = 52.0;
const MARGIN_BOTTOM: f32 = 36.0;
const HINT: &str = "DRAG: MOVE · CORNER / CTRL+WHEEL: SCALE · BOTTOM EDGE: CUT · DOUBLE-CLICK: FIT";
/// Hatching on the paper margins the print head does not reach.
const HATCH: Color32 = Color32::from_rgb(222, 222, 216);
const FULL_UV: Rect = Rect { min: Pos2::ZERO, max: pos2(1.0, 1.0) };

enum Msg {
    Status(String),
    Done(Result<(), String>),
}

/// What a drag on the paper changes.
#[derive(Clone, Copy, PartialEq)]
enum Target {
    Move,
    /// Image corner, clockwise from the top left.
    Corner(usize),
    Cut,
}

#[derive(Clone, Copy)]
struct Drag {
    target: Target,
    start: Placement,
    origin: Pos2,
}

/// Snap guide, in dots from the top-left of the print area.
enum Guide {
    X(f32),
    Y(f32),
}

/// Paper zoom (screen points per mm) and position. Frozen while the user drags or zooms,
/// otherwise the paper would rescale under the pointer as its length changes.
#[derive(Clone, Copy)]
struct View {
    scale: f32,
    offset: Vec2,
}

pub fn run(path: PathBuf) -> Result<(), String> {
    let img = image::open(&path).map_err(|e| format!("Could not open {}: {e}", path.display()))?;
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Kaminoneko · print")
            .with_app_id("kaminoneko")
            .with_inner_size([1240.0, 820.0])
            .with_min_inner_size([900.0, 560.0]),
        ..Default::default()
    };
    eframe::run_native(
        "kaminoneko",
        options,
        Box::new(move |cc| Ok(Box::new(Editor::new(cc, img, path)))),
    )
    .map_err(|e| e.to_string())
}

struct Editor {
    path: PathBuf,
    gray: GrayImage,
    settings: Settings,
    placement: Placement,
    // Render cache, one entry per stage of `imaging`.
    toned: GrayImage,
    toned_of: Option<Settings>,
    ghost_tex: Option<TextureHandle>,
    scaled: GrayImage,
    scaled_of: Option<(Settings, u32)>,
    scaled_fast: bool,
    sheet: GrayImage,
    sheet_of: Option<(Settings, Placement, bool)>,
    sheet_tex: Option<TextureHandle>,
    // Paper interaction.
    drag: Option<Drag>,
    view: Option<View>,
    hold_view_until: f64,
    // Printing.
    copies: u32,
    status: String,
    is_error: bool,
    job: Option<Receiver<Msg>>,
    cfg: Config,
}

impl Editor {
    fn new(cc: &eframe::CreationContext<'_>, img: image::DynamicImage, path: PathBuf) -> Self {
        theme::install(&cc.egui_ctx);
        let gray = img.to_luma8();
        // Dark-mode screenshots waste a lot of paper: invert them up front.
        let mean = gray.as_raw().iter().map(|&v| v as u64).sum::<u64>() / gray.as_raw().len().max(1) as u64;
        let dark = mean < 100;
        let status = if dark {
            "Dark screenshot: INVERT was turned on to save paper."
        } else {
            "Drag the image to move it; drag a corner to scale it."
        };
        Self {
            path,
            gray,
            settings: Settings { invert: dark, ..Settings::default() },
            placement: Placement::default(),
            toned: GrayImage::new(1, 1),
            toned_of: None,
            ghost_tex: None,
            scaled: GrayImage::new(1, 1),
            scaled_of: None,
            scaled_fast: false,
            sheet: GrayImage::new(1, 1),
            sheet_of: None,
            sheet_tex: None,
            drag: None,
            view: None,
            hold_view_until: 0.0,
            copies: 1,
            status: status.into(),
            is_error: false,
            job: None,
            cfg: Config::from_env(),
        }
    }

    fn printing(&self) -> bool {
        self.job.is_some()
    }

    /// Re-runs only the stages whose inputs changed. `fast` while the user drags.
    fn update_preview(&mut self, ctx: &egui::Context, fast: bool) {
        if self.toned_of != Some(self.settings) {
            self.toned = imaging::tone(&self.gray, &self.settings);
            let (w, h) = self.toned.dimensions();
            let ghost = if w.max(h) > GHOST_SIDE {
                imageops::thumbnail(&self.toned, w * GHOST_SIDE / w.max(h), h * GHOST_SIDE / w.max(h))
            } else {
                self.toned.clone()
            };
            self.ghost_tex = Some(ctx.load_texture("ghost", gray_image(&ghost), TextureOptions::LINEAR));
            self.toned_of = Some(self.settings);
            self.placement = self.placement.clamp(self.toned.dimensions());
        }
        let scaled_of = (self.settings, self.placement.width);
        if self.scaled_of != Some(scaled_of) || (self.scaled_fast && !fast) {
            self.scaled = imaging::scale(&self.toned, self.placement.width, fast);
            self.scaled_of = Some(scaled_of);
            self.scaled_fast = fast;
        }
        let sheet_of = (self.settings, self.placement, self.scaled_fast);
        if self.sheet_of != Some(sheet_of) {
            self.sheet = imaging::compose(&self.scaled, &self.placement);
            // Mipmaps keep text readable when the strip is shown smaller than one pixel per dot.
            let options = TextureOptions {
                magnification: TextureFilter::Linear,
                minification: TextureFilter::Linear,
                mipmap_mode: Some(TextureFilter::Linear),
                ..TextureOptions::LINEAR
            };
            self.sheet_tex = Some(ctx.load_texture("sheet", gray_image(&self.sheet), options));
            self.sheet_of = Some(sheet_of);
        }
    }

    fn poll_job(&mut self) {
        let Some(rx) = &self.job else { return };
        while let Ok(msg) = rx.try_recv() {
            match msg {
                Msg::Status(text) => {
                    self.status = text;
                    self.is_error = false;
                }
                Msg::Done(result) => {
                    match result {
                        Ok(()) => {
                            self.status = "Done! Printed.".into();
                            self.is_error = false;
                        }
                        Err(e) => {
                            self.status = e;
                            self.is_error = true;
                        }
                    }
                    self.job = None;
                    return;
                }
            }
        }
    }

    fn print(&mut self, ctx: &egui::Context) {
        let (tx, rx) = mpsc::channel();
        let (gray, settings, placement) = (self.gray.clone(), self.settings, self.placement);
        let (cfg, copies, ctx) = (self.cfg.clone(), self.copies, ctx.clone());
        thread::spawn(move || {
            let notify = |text: String| {
                let _ = tx.send(Msg::Status(text));
                ctx.request_repaint();
            };
            // Full quality, whatever the preview cache holds.
            let sheet = imaging::render(&gray, &settings, &placement);
            let result = printer::print(&cfg, &sheet, copies, &notify);
            let _ = tx.send(Msg::Done(result));
            ctx.request_repaint();
        });
        self.job = Some(rx);
        self.status = "Getting ready…".into();
        self.is_error = false;
    }

    fn save(&mut self) {
        let name = format!(
            "{}_print.png",
            self.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "screenshot".into())
        );
        let target = self.path.with_file_name(name);
        match imaging::render(&self.gray, &self.settings, &self.placement).save(&target) {
            Ok(()) => {
                self.status = format!("Saved to {}", target.display());
                self.is_error = false;
            }
            Err(e) => {
                self.status = format!("Could not save: {e}");
                self.is_error = true;
            }
        }
    }

    fn rotate(&mut self, quarter_turns: u8) {
        self.settings.rotation = (self.settings.rotation + quarter_turns) % 4;
        self.placement = Placement::default();
    }

    // ------------------------------------------------------------ bottom bar
    fn bottom_bar(&mut self, ui: &mut egui::Ui) {
        let busy = self.printing();
        let pal = theme::palette(ui.visuals().dark_mode);
        let size = self.toned.dimensions();

        // Image controls. Disabled while printing so the paper keeps showing what is printed.
        ui.add_enabled_ui(!busy, |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui.button("↺ 90°").on_hover_text("Rotate left").clicked() {
                    self.rotate(3);
                }
                if ui.button("↻ 90°").on_hover_text("Rotate right").clicked() {
                    self.rotate(1);
                }
                ui.separator();

                caption(ui, "SIZE");
                let p = &mut self.placement;
                let mut pct = (p.width * 100 + PAPER_WIDTH / 2) / PAPER_WIDTH;
                let slider = egui::Slider::new(&mut pct, 10..=400).logarithmic(true).suffix(" %");
                if ui.add(slider).on_hover_text("Image width, in % of the print width").changed() {
                    let center = p.x + p.width as i32 / 2;
                    p.width = pct * PAPER_WIDTH / 100;
                    p.x = center - p.width as i32 / 2;
                    *p = p.clamp(size);
                }
                if ui.button("FIT").on_hover_text("Full print width, at the top, paper ends with the image").clicked() {
                    *p = Placement::default();
                }
                if ui.button("CENTER").on_hover_text("Center horizontally").clicked() {
                    p.x = p.centered_x();
                }
                ui.separator();

                let s = &mut self.settings;
                caption(ui, "BRIGHTNESS");
                ui.add(egui::Slider::new(&mut s.brightness, -100..=100));
                ui.separator();
                caption(ui, "CONTRAST");
                ui.add(egui::Slider::new(&mut s.contrast, -50..=100));
                ui.separator();
                ui.checkbox(&mut s.auto_contrast, "AUTO CONTRAST");
                ui.checkbox(&mut s.invert, "INVERT")
                    .on_hover_text("Prints dark backgrounds as white (dark-mode screenshots)");
                ui.separator();
                if ui.button("RESET").on_hover_text("Reset every setting and the placement").clicked() {
                    self.settings = Settings::default();
                    self.placement = Placement::default();
                }
            });
        });

        ui.add_space(6.0);
        // Status on the left, actions on the right (added right to left).
        let (marker, text) = if self.is_error { (ERROR, ERROR) } else { (ACCENT, pal.ink) };
        let status = RichText::new(&self.status).color(text);
        let copies = &mut self.copies;
        let (_, (print, save)) = egui::Sides::new().shrink_left().show(
            ui,
            |ui| {
                if busy {
                    ui.spinner();
                } else {
                    ui.label(RichText::new("■").color(marker));
                }
                ui.add(egui::Label::new(status).truncate());
            },
            |ui| {
                let label = if busy { "PRINTING…" } else { "🖨 PRINT" };
                let print = theme::slab_button(ui, label, ACCENT, ON_ACCENT, !busy, vec2(170.0, 42.0)).clicked();
                let save = theme::slab_button(ui, "SAVE PNG", pal.panel, pal.ink, !busy, vec2(0.0, 42.0)).clicked();
                ui.add_enabled(!busy, egui::DragValue::new(copies).range(1..=10));
                caption(ui, "COPIES");
                (print, save)
            },
        );
        if print {
            let ctx = ui.ctx().clone();
            self.print(&ctx);
        }
        if save {
            self.save();
        }
    }

    // ----------------------------------------------------------------- paper
    fn paper(&mut self, ui: &mut egui::Ui) {
        let pal = theme::palette(ui.visuals().dark_mode);
        let ctx = ui.ctx().clone();
        let now = ui.input(|i| i.time);
        let fast = ui.input(|i| i.pointer.any_down()) || now < self.hold_view_until;
        self.update_preview(&ctx, fast);

        ui.horizontal(|ui| {
            ui.label(RichText::new("PAPER").font(FontId::new(15.0, theme::bold())).color(pal.ink));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.add(egui::Label::new(RichText::new(HINT).small().color(pal.weak)).truncate());
            });
        });

        let avail = ui.available_size();
        let holding = self.drag.is_some() || now < self.hold_view_until;
        if !holding || self.view.is_none() {
            self.view = Some(fit_view(self.placement.length(self.toned.dimensions()), avail));
        }
        let Some(view) = self.view else { return };
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| self.paper_canvas(ui, view, avail.y, &pal));

        if now < self.hold_view_until {
            ctx.request_repaint_after(Duration::from_secs_f64(self.hold_view_until - now + 0.05));
        }
    }

    fn paper_canvas(&mut self, ui: &mut egui::Ui, view: View, min_height: f32, pal: &Palette) {
        let size = self.toned.dimensions();
        let now = ui.input(|i| i.time);
        let k = view.scale / DOTS_PER_MM; // screen points per dot
        let lead = LEAD_MM * view.scale;
        let paper_w = PAPER_WIDTH_MM * view.scale;
        let length = self.placement.length(size) as f32;
        let height = (view.offset.y + length * k + 2.0 * lead + MARGIN_BOTTOM).max(min_height);
        let (response, painter) = ui.allocate_painter(vec2(ui.available_width(), height), Sense::click_and_drag());

        let paper_min = response.rect.min + view.offset;
        let print_min = paper_min + vec2((paper_w - PAPER_WIDTH as f32 * k) / 2.0, lead);
        let to_dots = |p: Pos2| (p - print_min) / k;
        // Image and paper rectangles on screen.
        let geometry = |pl: &Placement| {
            let image = Rect::from_min_size(
                print_min + vec2(pl.x as f32, pl.y as f32) * k,
                vec2(pl.width as f32, pl.height(size) as f32) * k,
            );
            let paper = Rect::from_min_size(paper_min, vec2(paper_w, pl.length(size) as f32 * k + 2.0 * lead));
            (image, paper)
        };

        // ---- input
        let mut guides = Vec::new();
        if !self.printing() {
            let (image, paper) = geometry(&self.placement);
            if response.drag_started()
                && let Some(origin) = ui.input(|i| i.pointer.press_origin())
            {
                let target = target_at(origin, image, paper);
                self.drag = Some(Drag { target, start: self.placement, origin });
            }
            if response.dragged()
                && let (Some(drag), Some(pos)) = (self.drag, response.interact_pointer_pos())
            {
                let (placement, g) = drag_to(drag, pos, k, size, &to_dots);
                self.placement = placement.clamp(size);
                guides = g;
            }
            if response.drag_stopped() {
                self.drag = None;
                self.hold_view_until = now + 0.3;
            }
            if response.double_clicked() {
                match response.interact_pointer_pos().map(|p| target_at(p, image, paper)) {
                    Some(Target::Cut) => self.placement.length = None,
                    _ => self.placement = Placement::default(),
                }
            }
            // Ctrl + wheel or pinch: scale around the pointer.
            let zoom = ui.input(|i| i.zoom_delta());
            if let Some(pos) = response.hover_pos()
                && zoom != 1.0
            {
                let p = &mut self.placement;
                let at = to_dots(pos);
                let width = (p.width as f32 * zoom).round();
                let z = width / p.width as f32;
                p.x = (at.x - (at.x - p.x as f32) * z).round() as i32;
                p.y = (at.y - (at.y - p.y as f32) * z).round() as i32;
                p.width = width as u32;
                *p = p.clamp(size);
                self.hold_view_until = now + 0.6;
            }
            let target = match (self.drag, response.hover_pos()) {
                (Some(d), _) => Some(d.target),
                (None, Some(pos)) => Some(target_at(pos, image, paper)),
                _ => None,
            };
            if let Some(target) = target {
                ui.ctx().set_cursor_icon(match target {
                    Target::Corner(0 | 2) => CursorIcon::ResizeNwSe,
                    Target::Corner(_) => CursorIcon::ResizeNeSw,
                    Target::Cut => CursorIcon::ResizeVertical,
                    Target::Move if self.drag.is_some() => CursorIcon::Grabbing,
                    Target::Move => CursorIcon::Grab,
                });
            }
        }
        let fast = ui.input(|i| i.pointer.any_down()) || now < self.hold_view_until;
        let ctx = ui.ctx().clone();
        self.update_preview(&ctx, fast);

        // ---- paint
        let (image, paper) = geometry(&self.placement);
        let print = Rect::from_min_size(print_min, vec2(PAPER_WIDTH as f32, self.sheet.height() as f32) * k);
        draw_grid(&painter, response.rect, paper_min, view.scale * 10.0, pal.grid);
        draw_paper(&painter, paper);
        for margin in [
            Rect::from_x_y_ranges(paper.min.x..=print.min.x, paper.y_range()),
            Rect::from_x_y_ranges(print.max.x..=paper.max.x, paper.y_range()),
        ] {
            hatch(&painter.with_clip_rect(margin.intersect(painter.clip_rect())), margin, HATCH);
        }
        if let Some(tex) = &self.ghost_tex {
            painter.image(tex.id(), image, FULL_UV, Color32::WHITE.gamma_multiply(0.22));
        }
        if let Some(tex) = &self.sheet_tex {
            painter.image(tex.id(), print, FULL_UV, PAPER);
        }
        // Length label right of the paper, or right of the image when it sticks out.
        let length_x = (paper.max.x.max(image.max.x) + 20.0).min(response.rect.max.x - 80.0);
        draw_dimensions(&painter, paper, print, length_x, view.scale, pal);

        let active = self.drag.is_some() || response.hovered() || cfg!(feature = "test-screenshot");
        if active && !self.printing() {
            let band = [print.left_top(), print.left_bottom()].map(|p| p.x);
            for x in [band[0], print.max.x] {
                painter.extend(Shape::dashed_line(
                    &[pos2(x, paper.min.y), pos2(x, paper.max.y)],
                    Stroke::new(1.0, pal.weak),
                    3.0,
                    3.0,
                ));
            }
            draw_frame(&painter, image);
            let (w_mm, h_mm) = (self.placement.width as f32 / DOTS_PER_MM, image.height() / view.scale);
            let pct = self.placement.width * 100 / PAPER_WIDTH;
            label_box(&painter, image.left_top() - vec2(0.0, 10.0), &format!("{pct}% · {w_mm:.0}×{h_mm:.0} MM"));
        }
        for guide in guides {
            let (a, b) = match guide {
                Guide::X(x) => {
                    let x = print_min.x + x * k;
                    (pos2(x, response.rect.min.y), pos2(x, response.rect.max.y))
                }
                Guide::Y(y) => {
                    let y = print_min.y + y * k;
                    (pos2(response.rect.min.x, y), pos2(response.rect.max.x, y))
                }
            };
            painter.line_segment([a, b], Stroke::new(1.0, GUIDE));
        }
    }
}

impl eframe::App for Editor {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_job();
        let pal = theme::palette(ui.visuals().dark_mode);
        egui::Panel::bottom("controls")
            .frame(egui::Frame::side_top_panel(ui.style()).inner_margin(egui::Margin::symmetric(12, 10)))
            .show(ui, |ui| self.bottom_bar(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(ui.style()).fill(pal.desk).inner_margin(12))
            .show(ui, |ui| self.paper(ui));

        #[cfg(feature = "test-screenshot")]
        test_screenshot(ui.ctx(), &mut self.placement);
    }
}

fn gray_image(img: &GrayImage) -> ColorImage {
    ColorImage::from_gray([img.width() as usize, img.height() as usize], img.as_raw())
}

/// Small bold label for the bottom bar.
fn caption(ui: &mut egui::Ui, text: &str) {
    ui.label(RichText::new(text).font(FontId::new(12.0, theme::bold())));
}

/// Largest zoom that shows the whole strip, up to one screen point per dot: bigger only
/// shows blocky dots. Long strips keep a readable width and scroll.
/// Long strips keep a readable width and scroll.
fn fit_view(length_dots: u32, avail: Vec2) -> View {
    let paper_mm = vec2(PAPER_WIDTH_MM, length_dots as f32 / DOTS_PER_MM + 2.0 * LEAD_MM);
    let room = avail - vec2(MARGIN_LEFT + MARGIN_RIGHT, MARGIN_TOP + MARGIN_BOTTOM);
    let fit_w = (room.x / paper_mm.x).max(0.5);
    let fit_h = room.y / paper_mm.y;
    let scale = fit_w.min(fit_h).min(DOTS_PER_MM).max(fit_w.min(3.0));
    let free = (room - paper_mm * scale).max(Vec2::ZERO);
    View { scale, offset: vec2(MARGIN_LEFT, MARGIN_TOP) + free / 2.0 }
}

fn target_at(pos: Pos2, image: Rect, paper: Rect) -> Target {
    let corners = [image.left_top(), image.right_top(), image.right_bottom(), image.left_bottom()];
    if let Some(i) = corners.iter().position(|c| c.distance(pos) <= HANDLE) {
        Target::Corner(i)
    } else if (pos.y - paper.max.y).abs() <= 6.0 && paper.x_range().contains(pos.x) {
        Target::Cut
    } else {
        Target::Move
    }
}

/// New placement for a drag at `pos`, snapped to the print area, plus the guides to draw.
fn drag_to(
    drag: Drag,
    pos: Pos2,
    k: f32,
    size: (u32, u32),
    to_dots: &dyn Fn(Pos2) -> Vec2,
) -> (Placement, Vec<Guide>) {
    let tolerance = SNAP / k;
    let band = PAPER_WIDTH as f32;
    let start = drag.start;
    let mut p = start;
    let mut guides = Vec::new();
    match drag.target {
        Target::Move => {
            let delta = (pos - drag.origin) / k;
            let (w, h) = (start.width as f32, start.height(size) as f32);
            let mut x = start.x as f32 + delta.x;
            let mut y = start.y as f32 + delta.y;
            // Center first, then the edges of the print area.
            for (edge, target) in [(x + w / 2.0, band / 2.0), (x, 0.0), (x + w, band)] {
                if (edge - target).abs() <= tolerance {
                    x += target - edge;
                    guides.push(Guide::X(target));
                    break;
                }
            }
            for (edge, target) in [(y, 0.0), (y + h, 0.0)] {
                if (edge - target).abs() <= tolerance {
                    y += target - edge;
                    guides.push(Guide::Y(target));
                    break;
                }
            }
            p.x = x.round() as i32;
            p.y = y.round() as i32;
        }
        Target::Corner(i) => {
            let (w, h) = (start.width as f32, start.height(size) as f32);
            let (x0, y0) = (start.x as f32, start.y as f32);
            let corners = [(x0, y0), (x0 + w, y0), (x0 + w, y0 + h), (x0, y0 + h)];
            let anchor = corners[(i + 2) % 4];
            let right = i == 1 || i == 2;
            let lower = i == 2 || i == 3;
            let at = to_dots(pos);
            let aspect = size.0 as f32 / size.1.max(1) as f32;
            let mut width = (at.x - anchor.0).abs().max((at.y - anchor.1).abs() * aspect);
            // Snap the moving side to the edges of the print area.
            let side = if right { anchor.0 + width } else { anchor.0 - width };
            for target in [0.0, band] {
                if (side - target).abs() <= tolerance && (target - anchor.0) * (side - anchor.0) > 0.0 {
                    width = (target - anchor.0).abs();
                    guides.push(Guide::X(target));
                }
            }
            let width = width.round().clamp(imaging::MIN_DOTS as f32, imaging::MAX_WIDTH as f32);
            let height = width / aspect;
            p.width = width as u32;
            p.x = (if right { anchor.0 } else { anchor.0 - width }).round() as i32;
            p.y = (if lower { anchor.1 } else { anchor.1 - height }).round() as i32;
        }
        Target::Cut => {
            let length = to_dots(pos).y - LEAD_MM * DOTS_PER_MM;
            let natural = (start.y + start.height(size) as i32).max(0) as f32;
            if (length - natural).abs() <= tolerance {
                p.length = None;
                guides.push(Guide::Y(natural));
            } else {
                p.length = Some(length.round().max(imaging::MIN_DOTS as f32) as u32);
            }
        }
    }
    (p, guides)
}

/// Cutting-mat grid behind the paper, one line per cm, aligned with the paper.
fn draw_grid(painter: &Painter, area: Rect, origin: Pos2, step: f32, color: Color32) {
    if step < 10.0 {
        return;
    }
    let stroke = Stroke::new(1.0, color);
    let mut x = origin.x - ((origin.x - area.min.x) / step).floor() * step;
    while x <= area.max.x {
        painter.vline(x, area.y_range(), stroke);
        x += step;
    }
    let mut y = origin.y - ((origin.y - area.min.y) / step).floor() * step;
    while y <= area.max.y {
        painter.hline(area.x_range(), y, stroke);
        y += step;
    }
}

/// Diagonal lines over `area`: paper that never gets printed.
fn hatch(painter: &Painter, area: Rect, color: Color32) {
    let stroke = Stroke::new(1.0, color);
    let mut d = 0.0;
    while d < area.width() + area.height() {
        let a = pos2(area.min.x + d, area.min.y);
        painter.line_segment([a, a + vec2(-area.height(), area.height())], stroke);
        d += 7.0;
    }
}

/// Paper strip with torn edges, a black outline and a hard shadow.
fn draw_paper(painter: &Painter, paper: Rect) {
    painter.extend(strip(paper.translate(vec2(8.0, 8.0)), Color32::BLACK));
    painter.extend(strip(paper, PAPER));
    let (teeth, tw, th) = teeth(paper);
    let (top, bottom) = (paper.min.y, paper.max.y);
    let mut outline = Vec::with_capacity(teeth * 4 + 2);
    for i in 0..teeth {
        let x = paper.min.x + i as f32 * tw;
        outline.extend([pos2(x, top), pos2(x + tw / 2.0, top - th)]);
    }
    outline.push(pos2(paper.max.x, top));
    for i in (0..teeth).rev() {
        let x = paper.min.x + i as f32 * tw;
        outline.extend([pos2(x + tw, bottom), pos2(x + tw / 2.0, bottom + th)]);
    }
    outline.push(pos2(paper.min.x, bottom));
    painter.add(Shape::closed_line(outline, Stroke::new(2.0, Color32::BLACK)));
}

fn teeth(paper: Rect) -> (usize, f32, f32) {
    let n = (paper.width() / 10.0).round().max(1.0);
    (n as usize, paper.width() / n, 4.0)
}

/// Filled strip: a rectangle plus one triangle per tooth (each piece is convex).
fn strip(paper: Rect, color: Color32) -> Vec<Shape> {
    let (n, tw, th) = teeth(paper);
    let (top, bottom) = (paper.min.y, paper.max.y);
    let mut shapes = vec![Shape::rect_filled(paper, 0.0, color)];
    for i in 0..n {
        let x = paper.min.x + i as f32 * tw;
        // The 0.5 px overlap hides the anti-aliasing seam between the teeth and the strip.
        shapes.push(Shape::convex_polygon(
            vec![pos2(x, top + 0.5), pos2(x + tw / 2.0, top - th), pos2(x + tw, top + 0.5)],
            color,
            Stroke::NONE,
        ));
        shapes.push(Shape::convex_polygon(
            vec![pos2(x + tw, bottom - 0.5), pos2(x + tw / 2.0, bottom + th), pos2(x, bottom - 0.5)],
            color,
            Stroke::NONE,
        ));
    }
    shapes
}

/// Paper width above, print length on the right, cm ruler on the left.
fn draw_dimensions(painter: &Painter, paper: Rect, print: Rect, length_x: f32, scale: f32, pal: &Palette) {
    let line = Stroke::new(2.0, pal.ink);
    let thin = Stroke::new(1.0, pal.ink);
    let big = FontId::new(14.0, theme::bold());
    let small = FontId::new(11.0, theme::bold());

    let y = paper.min.y - 22.0;
    dimension(painter, [pos2(paper.min.x, y), pos2(paper.max.x, y)], line);
    painter.text(pos2(paper.center().x, y - 5.0), Align2::CENTER_BOTTOM, format!("{PAPER_WIDTH_MM:.0} MM"), big.clone(), pal.ink);

    let x = length_x;
    dimension(painter, [pos2(x, print.min.y), pos2(x, print.max.y)], line);
    let length_mm = print.height() / scale;
    painter.text(pos2(x + 10.0, print.center().y), Align2::LEFT_CENTER, format!("{:.1} CM", length_mm / 10.0), big, pal.ink);

    let x = paper.min.x - 12.0;
    painter.line_segment([pos2(x, print.min.y), pos2(x, print.max.y)], line);
    let step = if scale < 2.0 { 5 } else { 1 };
    for mm in (0..=length_mm.floor() as u32).step_by(step) {
        let y = print.min.y + mm as f32 * scale;
        let len = if mm % 10 == 0 { 9.0 } else if mm % 5 == 0 { 6.0 } else { 3.0 };
        painter.line_segment([pos2(x - len, y), pos2(x, y)], if mm % 10 == 0 { line } else { thin });
        if mm % 10 == 0 {
            painter.text(pos2(x - 13.0, y), Align2::RIGHT_CENTER, (mm / 10).to_string(), small.clone(), pal.ink);
        }
    }
    painter.text(pos2(x, print.min.y - 8.0), Align2::RIGHT_BOTTOM, "CM", small, pal.ink);
}

/// Dimension line with a tick at each end.
fn dimension(painter: &Painter, [a, b]: [Pos2; 2], stroke: Stroke) {
    painter.line_segment([a, b], stroke);
    let tick = (b - a).normalized().rot90() * 6.0;
    for p in [a, b] {
        painter.line_segment([p - tick, p + tick], stroke);
    }
}

/// Hazard-striped outline around the image with square corner handles.
fn draw_frame(painter: &Painter, image: Rect) {
    let corners = [image.left_top(), image.right_top(), image.right_bottom(), image.left_bottom()];
    painter.add(Shape::closed_line(corners.to_vec(), Stroke::new(3.0, ON_ACCENT)));
    let mut path = corners.to_vec();
    path.push(corners[0]);
    painter.extend(Shape::dashed_line(&path, Stroke::new(3.0, ACCENT), 8.0, 8.0));
    for c in corners {
        let handle = Rect::from_center_size(c, Vec2::splat(12.0));
        painter.rect(handle, 0.0, ACCENT, Stroke::new(2.0, ON_ACCENT), egui::StrokeKind::Inside);
    }
}

/// Black tag with yellow text, bottom-left corner at `pos`.
fn label_box(painter: &Painter, pos: Pos2, text: &str) {
    let galley = painter.layout_no_wrap(text.to_owned(), FontId::new(12.0, theme::bold()), ACCENT);
    let rect = Rect::from_min_size(pos - vec2(0.0, galley.size().y + 6.0), galley.size() + vec2(12.0, 6.0));
    painter.rect_filled(rect, 0.0, ON_ACCENT);
    painter.galley(rect.min + vec2(6.0, 3.0), galley, ACCENT);
}

/// Testing only: KAMINONEKO_TEST_SCREENSHOT=file.png saves the window and exits.
/// KAMINONEKO_TEST_PLACEMENT=width,x,y places the image first.
#[cfg(feature = "test-screenshot")]
fn test_screenshot(ctx: &egui::Context, placement: &mut Placement) {
    let Ok(path) = std::env::var("KAMINONEKO_TEST_SCREENSHOT") else { return };
    let frame = ctx.cumulative_frame_nr();
    if frame == 1 && std::env::var("KAMINONEKO_TEST_THEME").as_deref() == Ok("light") {
        ctx.set_theme(egui::Theme::Light);
    }
    if frame == 1
        && let Ok(spec) = std::env::var("KAMINONEKO_TEST_PLACEMENT")
    {
        let v: Vec<i32> = spec.split(',').filter_map(|s| s.trim().parse().ok()).collect();
        if let [w, x, y, ref rest @ ..] = v[..] {
            *placement = Placement { width: w as u32, x, y, length: rest.first().map(|&l| l as u32) };
        }
    }
    if frame == 15 {
        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::ScreenshotCallback::new(move |img| {
            let rgba: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
            let _ = image::RgbaImage::from_raw(img.size[0] as u32, img.size[1] as u32, rgba).map(|i| i.save(&path));
            std::process::exit(0);
        })));
    }
    ctx.request_repaint();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Source image 2:1; one screen point per dot, print area at the origin.
    const SIZE: (u32, u32) = (200, 100);
    fn to_dots(p: Pos2) -> Vec2 {
        p.to_vec2()
    }
    fn start() -> Placement {
        Placement { width: 200, x: 50, y: 20, length: None }
    }

    #[test]
    fn move_follows_the_pointer() {
        let drag = Drag { target: Target::Move, start: start(), origin: pos2(100.0, 50.0) };
        let (p, guides) = drag_to(drag, pos2(130.0, 90.0), 1.0, SIZE, &to_dots);
        assert_eq!((p.x, p.y), (80, 60));
        assert!(guides.is_empty());
    }

    #[test]
    fn move_snaps_to_center_and_top() {
        let drag = Drag { target: Target::Move, start: start(), origin: Pos2::ZERO };
        let (p, guides) = drag_to(drag, pos2(44.0, -17.0), 1.0, SIZE, &to_dots);
        assert_eq!((p.x, p.y), (92, 0)); // (384 - 200) / 2
        assert!(matches!(guides[..], [Guide::X(192.0), Guide::Y(0.0)]));
    }

    #[test]
    fn corner_scales_around_the_opposite_corner() {
        // Bottom-right corner: the top-left corner stays put, the aspect ratio is kept.
        let drag = Drag { target: Target::Corner(2), start: start(), origin: pos2(250.0, 120.0) };
        let (p, _) = drag_to(drag, pos2(350.0, 170.0), 1.0, SIZE, &to_dots);
        assert_eq!((p.width, p.x, p.y), (300, 50, 20));
        // Top-left corner: the bottom-right corner (250, 120) stays put.
        let drag = Drag { target: Target::Corner(0), start: start(), origin: pos2(50.0, 20.0) };
        let (p, _) = drag_to(drag, pos2(150.0, 70.0), 1.0, SIZE, &to_dots);
        assert_eq!((p.width, p.x, p.y), (100, 150, 70));
        assert_eq!(p.height(SIZE), 50);
    }

    #[test]
    fn corner_snaps_to_the_print_edge() {
        let drag = Drag { target: Target::Corner(1), start: start(), origin: pos2(250.0, 20.0) };
        let (p, guides) = drag_to(drag, pos2(380.0, 20.0), 1.0, SIZE, &to_dots);
        assert_eq!((p.width, p.x), (334, 50)); // right side exactly on 384
        assert!(matches!(guides[..], [Guide::X(384.0)]));
    }

    #[test]
    fn cut_sets_the_length_and_snaps_back_to_auto() {
        let lead = LEAD_MM * DOTS_PER_MM;
        let drag = Drag { target: Target::Cut, start: start(), origin: Pos2::ZERO };
        let (p, _) = drag_to(drag, pos2(10.0, lead + 60.0), 1.0, SIZE, &to_dots);
        assert_eq!(p.length, Some(60));
        // Natural end: y 20 + height 100.
        let (p, guides) = drag_to(drag, pos2(10.0, lead + 117.0), 1.0, SIZE, &to_dots);
        assert_eq!(p.length, None);
        assert!(matches!(guides[..], [Guide::Y(120.0)]));
    }

    #[test]
    fn snapping_distance_is_in_screen_points() {
        // At 4 points per dot, 7 points of tolerance is under 2 dots.
        let drag = Drag { target: Target::Move, start: start(), origin: Pos2::ZERO };
        let (p, guides) = drag_to(drag, pos2(-4.0 * 3.0, 0.0), 4.0, SIZE, &to_dots);
        assert_eq!(p.x, 47);
        assert!(guides.is_empty());
    }
}
