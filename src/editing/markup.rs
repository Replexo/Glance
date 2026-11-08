// Copyright (c) 2026 Replexo
//
// This program is free software: you can redistribute it and/or modify
// it under the terms of the GNU General Public License as published by
// the Free Software Foundation, either version 3 of the License, or
// (at your option) any later version.
//
// This program is distributed in the hope that it will be useful,
// but WITHOUT ANY WARRANTY; without even the implied warranty of
// MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
// GNU General Public License for more details.
//
// You should have received a copy of the GNU General Public License
// along with this program.  If not, see <http://www.gnu.org/licenses/>.
//
// SPDX-License-Identifier: GPL-3.0-or-later

//! Markup shapes drawn on top of the image
//!
//! Shapes live in the pixel coordinates of the original image, with its
//! orientation applied. Crop, rotate and flip are added on top as one matrix
//! from [`operations_matrix`], so shapes stay on the same part of the picture
//! when those operations are added, reset or undone. A new shape gets the
//! inverse of the current matrix, so it is drawn upright in the view it was
//! drawn in. The same cairo drawing code renders the live view and the saved
//! image.

use glycin::{Operation, Operations};
use gufo::common::orientation::Rotation;
use image::RgbaImage;

use crate::deps::*;

/// Markup tools
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tool {
    /// Select and move drawn shapes
    Select,
    #[default]
    Brush,
    Highlighter,
    Line,
    Arrow,
    Rectangle,
    Ellipse,
    Text,
}

/// Colours offered in the colour popover: blue, yellow, red, orange, green
pub const COLORS: [Color; 5] = [
    Color::rgb(0x35, 0x84, 0xe4),
    Color::rgb(0xf6, 0xd3, 0x2d),
    Color::rgb(0xe0, 0x1b, 0x24),
    Color::rgb(0xff, 0x78, 0x00),
    Color::rgb(0x33, 0xd1, 0x7a),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Color {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Color {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Self {
        Self { r, g, b }
    }

    fn set_source(&self, cr: &cairo::Context, alpha: f64) {
        cr.set_source_rgba(
            self.r as f64 / 255.,
            self.g as f64 / 255.,
            self.b as f64 / 255.,
            alpha,
        );
    }
}

impl Default for Color {
    fn default() -> Self {
        COLORS[0]
    }
}

/// Opacity of the highlighter
const HIGHLIGHTER_ALPHA: f64 = 0.4;

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    /// Freehand line, drawn with the brush or highlighter
    Stroke {
        points: Vec<(f64, f64)>,
        highlighter: bool,
    },
    Line {
        from: (f64, f64),
        to: (f64, f64),
    },
    Rectangle {
        from: (f64, f64),
        to: (f64, f64),
    },
    Ellipse {
        from: (f64, f64),
        to: (f64, f64),
    },
    Arrow {
        from: (f64, f64),
        to: (f64, f64),
    },
    Text {
        /// Top left corner of the text
        position: (f64, f64),
        text: String,
    },
}

/// One drawn element
#[derive(Debug, Clone, PartialEq)]
pub struct Shape {
    pub kind: Kind,
    pub color: Color,
    /// Line width, or font size for text, in shape coordinates
    pub size: f64,
    /// Maps shape coordinates to original image coordinates
    pub matrix: cairo::Matrix,
    /// Rectangles and ellipses are filled instead of outlined
    pub filled: bool,
}

impl Shape {
    pub fn new(kind: Kind, color: Color, size: f64) -> Self {
        Self {
            kind,
            color,
            size,
            matrix: cairo::Matrix::identity(),
            filled: false,
        }
    }
}

impl Shape {
    /// Bounding box in shape coordinates, including the line width
    pub fn bounds(&self) -> (f64, f64, f64, f64) {
        let points: Vec<(f64, f64)> = match &self.kind {
            Kind::Stroke { points, .. } => points.clone(),
            Kind::Line { from, to }
            | Kind::Rectangle { from, to }
            | Kind::Ellipse { from, to }
            | Kind::Arrow { from, to } => vec![*from, *to],
            Kind::Text { position, text } => {
                let (width, height) = text_size(text, self.size);
                return (
                    position.0,
                    position.1,
                    position.0 + width,
                    position.1 + height,
                );
            }
        };

        // Arrow heads reach out about twice the line width
        let pad = match self.kind {
            Kind::Arrow { .. } => self.size * 2.,
            _ => self.size / 2.,
        };
        let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
        for (x, y) in points {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
        (x0 - pad, y0 - pad, x1 + pad, y1 + pad)
    }

    /// Whether `point` in shape coordinates touches the shape, with
    /// `tolerance` added to the line width
    pub fn hit(&self, point: (f64, f64), tolerance: f64) -> bool {
        let reach = self.size / 2. + tolerance;
        match &self.kind {
            Kind::Stroke { points, .. } => {
                points.len() == 1 && distance(points[0], point) <= reach
                    || points
                        .windows(2)
                        .any(|w| segment_distance(w[0], w[1], point) <= reach)
            }
            Kind::Line { from, to } => segment_distance(*from, *to, point) <= reach,
            Kind::Arrow { from, to } => {
                segment_distance(*from, *to, point) <= self.size * 2. + tolerance
            }
            Kind::Rectangle { from, to }
                if self.filled
                    && point.0 >= from.0.min(to.0) - reach
                    && point.0 <= from.0.max(to.0) + reach
                    && point.1 >= from.1.min(to.1) - reach
                    && point.1 <= from.1.max(to.1) + reach =>
            {
                true
            }
            Kind::Rectangle { from, to } => {
                let corners = [*from, (to.0, from.1), *to, (from.0, to.1), *from];
                corners
                    .windows(2)
                    .any(|w| segment_distance(w[0], w[1], point) <= reach)
            }
            Kind::Ellipse { from, to } => {
                let center = ((from.0 + to.0) / 2., (from.1 + to.1) / 2.);
                let (rx, ry) = ((to.0 - from.0).abs() / 2., (to.1 - from.1).abs() / 2.);
                if rx <= 0. || ry <= 0. {
                    return distance(center, point) <= reach;
                }
                // Distance along the ray from the centre, close enough for
                // picking
                let (dx, dy) = (point.0 - center.0, point.1 - center.1);
                let norm = ((dx / rx).powi(2) + (dy / ry).powi(2)).sqrt();
                let length = dx.hypot(dy);
                norm <= 1. && self.filled || norm > 0. && (length - length / norm).abs() <= reach
            }
            Kind::Text { .. } => {
                let (x0, y0, x1, y1) = self.bounds();
                point.0 >= x0 - tolerance
                    && point.0 <= x1 + tolerance
                    && point.1 >= y0 - tolerance
                    && point.1 <= y1 + tolerance
            }
        }
    }

    /// Moves the shape by `(dx, dy)` in original image coordinates
    pub fn translate(&mut self, dx: f64, dy: f64) {
        self.matrix =
            cairo::Matrix::multiply(&self.matrix, &cairo::Matrix::new(1., 0., 0., 1., dx, dy));
    }
}

/// Font and options for text shapes
///
/// Glyph positions are the same at every scale, so the view matches the
/// saved image.
fn setup_text_layout(layout: &pango::Layout, text: &str, size: f64) {
    let context = layout.context();
    if let Ok(mut options) = cairo::FontOptions::new() {
        options.set_hint_metrics(cairo::HintMetrics::Off);
        options.set_hint_style(cairo::HintStyle::None);
        pangocairo::functions::context_set_font_options(&context, Some(&options));
    }
    context.set_round_glyph_positions(false);
    layout.context_changed();
    let mut font = pango::FontDescription::from_string("Sans Bold");
    font.set_absolute_size(size * pango::SCALE as f64);
    layout.set_font_description(Some(&font));
    layout.set_text(text);
}

/// Layout of a text shape, set up like it is drawn
fn text_layout(text: &str, size: f64) -> pango::Layout {
    let context = pango::prelude::FontMapExt::create_context(&pangocairo::FontMap::default());
    let layout = pango::Layout::new(&context);
    setup_text_layout(&layout, text, size);
    layout
}

/// Size of a text shape, measured like it is drawn
pub fn text_size(text: &str, size: f64) -> (f64, f64) {
    let (_, logical) = text_layout(text, size).extents();
    (
        logical.width() as f64 / pango::SCALE as f64,
        logical.height() as f64 / pango::SCALE as f64,
    )
}

/// Caret at byte `index` of `text`, as top and bottom point relative to
/// the top left corner of the text
pub fn text_caret(text: &str, size: f64, index: usize) -> ((f64, f64), (f64, f64)) {
    let layout = text_layout(text, size);
    let (strong, _) = layout.cursor_pos(index.min(text.len()) as i32);
    let scale = pango::SCALE as f64;
    let x = strong.x() as f64 / scale;
    let y = strong.y() as f64 / scale;
    ((x, y), (x, y + strong.height() as f64 / scale))
}

/// Byte index in `text` closest to `point`, relative to the top left corner
/// of the text
pub fn text_index_at(text: &str, size: f64, point: (f64, f64)) -> usize {
    let layout = text_layout(text, size);
    let scale = pango::SCALE as f64;
    let (_, index, trailing) =
        layout.xy_to_index((point.0 * scale) as i32, (point.1 * scale) as i32);
    let mut index = index.max(0) as usize;
    // `trailing` counts characters after `index`
    for _ in 0..trailing {
        index += text[index..].chars().next().map_or(0, char::len_utf8);
    }
    index.min(text.len())
}

fn distance(a: (f64, f64), b: (f64, f64)) -> f64 {
    (a.0 - b.0).hypot(a.1 - b.1)
}

fn segment_distance(a: (f64, f64), b: (f64, f64), p: (f64, f64)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let length = dx * dx + dy * dy;
    if length == 0. {
        return distance(a, p);
    }
    let t = (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / length).clamp(0., 1.);
    distance((a.0 + t * dx, a.1 + t * dy), p)
}

/// Line width for brush, rectangle and arrow on an image of this size
pub fn line_width(width: u32, height: u32) -> f64 {
    (width.max(height) as f64 * 0.006).max(2.)
}

/// Line width for the highlighter
pub fn highlighter_width(width: u32, height: u32) -> f64 {
    line_width(width, height) * 4.
}

/// Font size for text
pub fn font_size(width: u32, height: u32) -> f64 {
    (width.max(height) as f64 * 0.035).max(12.)
}

/// Draws `shapes` with `cr` set up in image coordinates
pub fn draw(cr: &cairo::Context, shapes: &[Shape]) {
    for shape in shapes {
        if let Err(err) = draw_shape(cr, shape) {
            tracing::warn!("Failed to draw shape: {err}");
        }
    }
}

fn draw_shape(cr: &cairo::Context, shape: &Shape) -> Result<(), cairo::Error> {
    cr.save()?;
    cr.transform(shape.matrix);
    cr.set_line_join(cairo::LineJoin::Round);
    cr.set_line_cap(cairo::LineCap::Round);
    cr.set_line_width(shape.size);

    match &shape.kind {
        Kind::Stroke {
            points,
            highlighter,
        } => {
            if let Some((first, rest)) = points.split_first() {
                if *highlighter {
                    shape.color.set_source(cr, HIGHLIGHTER_ALPHA);
                    cr.set_line_cap(cairo::LineCap::Square);
                } else {
                    shape.color.set_source(cr, 1.);
                }
                cr.move_to(first.0, first.1);
                if rest.is_empty() {
                    // A single click leaves a dot
                    cr.line_to(first.0, first.1);
                }
                for point in rest {
                    cr.line_to(point.0, point.1);
                }
                cr.stroke()?;
            }
        }
        Kind::Line { from, to } => {
            shape.color.set_source(cr, 1.);
            cr.move_to(from.0, from.1);
            cr.line_to(to.0, to.1);
            cr.stroke()?;
        }
        Kind::Ellipse { from, to } => {
            let (rx, ry) = ((to.0 - from.0).abs() / 2., (to.1 - from.1).abs() / 2.);
            if rx > 0. && ry > 0. {
                shape.color.set_source(cr, 1.);
                cr.save()?;
                cr.translate((from.0 + to.0) / 2., (from.1 + to.1) / 2.);
                cr.scale(rx, ry);
                cr.arc(0., 0., 1., 0., std::f64::consts::TAU);
                // Stroke with an unscaled pen
                cr.restore()?;
                if shape.filled {
                    cr.fill_preserve()?;
                }
                cr.stroke()?;
            }
        }
        Kind::Rectangle { from, to } => {
            shape.color.set_source(cr, 1.);
            cr.set_line_cap(cairo::LineCap::Square);
            cr.set_line_join(cairo::LineJoin::Miter);
            cr.rectangle(
                from.0.min(to.0),
                from.1.min(to.1),
                (to.0 - from.0).abs(),
                (to.1 - from.1).abs(),
            );
            if shape.filled {
                cr.fill_preserve()?;
            }
            cr.stroke()?;
        }
        Kind::Arrow { from, to } => {
            shape.color.set_source(cr, 1.);
            let (dx, dy) = (to.0 - from.0, to.1 - from.1);
            let length = dx.hypot(dy);
            if length > 0. {
                let head = (shape.size * 4.).min(length * 0.6);
                let angle = dy.atan2(dx);
                let spread = 0.45;
                // Stop the shaft inside the head so the tip stays sharp
                let shaft = (length - head * 0.6) / length;
                cr.move_to(from.0, from.1);
                cr.line_to(from.0 + dx * shaft, from.1 + dy * shaft);
                cr.stroke()?;

                cr.move_to(to.0, to.1);
                cr.line_to(
                    to.0 - head * (angle - spread).cos(),
                    to.1 - head * (angle - spread).sin(),
                );
                cr.line_to(
                    to.0 - head * (angle + spread).cos(),
                    to.1 - head * (angle + spread).sin(),
                );
                cr.close_path();
                cr.fill_preserve()?;
                cr.set_line_width(shape.size * 0.5);
                cr.stroke()?;
            }
        }
        Kind::Text { position, text } => {
            shape.color.set_source(cr, 1.);
            let layout = pangocairo::functions::create_layout(cr);
            setup_text_layout(&layout, text, shape.size);
            cr.move_to(position.0, position.1);
            pangocairo::functions::show_layout(cr, &layout);
        }
    }

    cr.restore()
}

/// Matrix from original image coordinates to coordinates after
/// `operations`, and the image size after them
pub fn operations_matrix(
    operations: Option<&Operations>,
    size: (f64, f64),
) -> (cairo::Matrix, (f64, f64)) {
    let mut matrix = cairo::Matrix::identity();
    let mut size = size;

    for operation in operations.map(|x| x.operations()).unwrap_or_default() {
        let (step, new_size) = operation_step(operation, size);
        matrix = cairo::Matrix::multiply(&matrix, &step);
        size = new_size;
    }

    (matrix, size)
}

/// Matrix for one operation on an image of `width` × `height` pixels
fn operation_step(
    operation: &Operation,
    (width, height): (f64, f64),
) -> (cairo::Matrix, (f64, f64)) {
    match operation {
        Operation::Clip((x, y, w, h)) => (
            cairo::Matrix::new(1., 0., 0., 1., -(*x as f64), -(*y as f64)),
            (*w as f64, *h as f64),
        ),
        Operation::MirrorHorizontally => (
            cairo::Matrix::new(-1., 0., 0., 1., width, 0.),
            (width, height),
        ),
        Operation::MirrorVertically => (
            cairo::Matrix::new(1., 0., 0., -1., 0., height),
            (width, height),
        ),
        Operation::Rotate(rotation) => match rotation {
            Rotation::_0 => (cairo::Matrix::identity(), (width, height)),
            // Counter-clockwise: (x, y) -> (y, width - x)
            Rotation::_90 => (
                cairo::Matrix::new(0., -1., 1., 0., 0., width),
                (height, width),
            ),
            Rotation::_180 => (
                cairo::Matrix::new(-1., 0., 0., -1., width, height),
                (width, height),
            ),
            // Clockwise: (x, y) -> (height - y, x)
            Rotation::_270 => (
                cairo::Matrix::new(0., 1., -1., 0., height, 0.),
                (height, width),
            ),
        },
        _ => {
            tracing::warn!("Markup can't follow operation {operation:?}");
            (cairo::Matrix::identity(), (width, height))
        }
    }
}

/// Draws the shapes into straight-alpha RGBA pixels
///
/// `matrix` maps shape coordinates to the pixels, see [`operations_matrix`].
pub fn flatten(
    image: &mut RgbaImage,
    shapes: &[Shape],
    matrix: cairo::Matrix,
) -> anyhow::Result<()> {
    if shapes.is_empty() {
        return Ok(());
    }

    // Area the shapes cover, to only touch those pixels
    let recording = cairo::RecordingSurface::create(cairo::Content::ColorAlpha, None)?;
    {
        let cr = cairo::Context::new(&recording)?;
        cr.set_matrix(matrix);
        draw(&cr, shapes);
    }
    let (ink_x, ink_y, ink_width, ink_height) = recording.ink_extents();

    let (width, height) = image.dimensions();
    let x0 = (ink_x.floor().max(0.) as u32).min(width);
    let y0 = (ink_y.floor().max(0.) as u32).min(height);
    let x1 = ((ink_x + ink_width).ceil().max(0.) as u32).min(width);
    let y1 = ((ink_y + ink_height).ceil().max(0.) as u32).min(height);

    // Cairo surfaces are limited to 32767 pixels per side
    for tile_y in (y0..y1).step_by(TILE_SIZE as usize) {
        for tile_x in (x0..x1).step_by(TILE_SIZE as usize) {
            let tile_width = TILE_SIZE.min(x1 - tile_x);
            let tile_height = TILE_SIZE.min(y1 - tile_y);
            flatten_tile(
                image,
                shapes,
                matrix,
                (tile_x, tile_y, tile_width, tile_height),
            )?;
        }
    }

    Ok(())
}

/// Edge length of the areas drawn at once
const TILE_SIZE: u32 = 2048;

fn flatten_tile(
    image: &mut RgbaImage,
    shapes: &[Shape],
    matrix: cairo::Matrix,
    (tile_x, tile_y, tile_width, tile_height): (u32, u32, u32, u32),
) -> anyhow::Result<()> {
    let mut surface =
        cairo::ImageSurface::create(cairo::Format::ARgb32, tile_width as i32, tile_height as i32)?;
    let stride = surface.stride() as usize;
    let mut before = Vec::with_capacity((tile_width * tile_height) as usize);

    {
        let mut data = surface.data()?;
        for y in 0..tile_height {
            for x in 0..tile_width {
                let [r, g, b, a] = image.get_pixel(tile_x + x, tile_y + y).0;
                let premultiply = |c: u8| ((c as u32 * a as u32 + 127) / 255) as u8;
                let argb = u32::from(a) << 24
                    | u32::from(premultiply(r)) << 16
                    | u32::from(premultiply(g)) << 8
                    | u32::from(premultiply(b));
                let offset = y as usize * stride + x as usize * 4;
                data[offset..offset + 4].copy_from_slice(&argb.to_ne_bytes());
                before.push(argb);
            }
        }
    }

    {
        let cr = cairo::Context::new(&surface)?;
        cr.translate(-(tile_x as f64), -(tile_y as f64));
        cr.transform(matrix);
        draw(&cr, shapes);
    }
    surface.flush();

    let data = surface.data()?;
    for y in 0..tile_height {
        for x in 0..tile_width {
            let offset = y as usize * stride + x as usize * 4;
            let argb = u32::from_ne_bytes(data[offset..offset + 4].try_into().unwrap());
            // Untouched pixels keep their exact straight-alpha values
            if argb == before[(y * tile_width + x) as usize] {
                continue;
            }

            let a = (argb >> 24) as u8;
            let unpremultiply = |c: u32| {
                if a == 0 {
                    0
                } else {
                    (((c & 0xff) * 255 + a as u32 / 2) / a as u32).min(255)
                }
            };
            image.put_pixel(
                tile_x + x,
                tile_y + y,
                image::Rgba([
                    unpremultiply(argb >> 16) as u8,
                    unpremultiply(argb >> 8) as u8,
                    unpremultiply(argb) as u8,
                    a,
                ]),
            );
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn transformed(matrix: &cairo::Matrix, point: (f64, f64)) -> (i64, i64) {
        let (x, y) = matrix.transform_point(point.0, point.1);
        (x.round() as i64, y.round() as i64)
    }

    fn ops(operations: Vec<Operation>) -> Operations {
        Operations::new(operations)
    }

    /// Follows a point through operations the same way `render` replays
    /// them on pixels
    #[test]
    fn operations_move_shapes_with_the_picture() {
        // 4×3 image, point (1, 0)
        let cases = [
            (Operation::Clip((1, 0, 2, 2)), (0, 0), (2., 2.)),
            (Operation::MirrorHorizontally, (3, 0), (4., 3.)),
            (Operation::MirrorVertically, (1, 3), (4., 3.)),
            // Counter-clockwise: top row becomes left column, read upwards
            (Operation::Rotate(Rotation::_90), (0, 3), (3., 4.)),
            (Operation::Rotate(Rotation::_270), (3, 1), (3., 4.)),
            (Operation::Rotate(Rotation::_180), (3, 3), (4., 3.)),
        ];

        for (operation, expected, expected_size) in cases {
            let (matrix, size) = operations_matrix(Some(&ops(vec![operation.clone()])), (4., 3.));
            assert_eq!(size, expected_size, "{operation:?}");
            assert_eq!(transformed(&matrix, (1., 0.)), expected, "{operation:?}");
        }
    }

    #[test]
    fn text_caret_and_index_agree() {
        let text = "Hä\nyo";
        let size = 40.;
        let (width, height) = text_size(text, size);
        assert!(width > 0. && height > 0.);

        // Start of text is the top left corner
        let ((x, y), (_, bottom)) = text_caret(text, size, 0);
        assert_eq!((x, y), (0., 0.));
        assert!(bottom > y);

        // Each character boundary maps back to itself
        for index in [0, 1, 3, 4, 5, 6] {
            let ((x, y), (_, bottom)) = text_caret(text, size, index);
            let found = text_index_at(text, size, (x + 0.5, (y + bottom) / 2.));
            assert_eq!(found, index, "index {index}");
        }

        // Empty text still has a line to type into
        let ((_, top), (_, bottom)) = text_caret("", size, 0);
        assert!(bottom - top > size * 0.5);
        assert_eq!(text_index_at("", size, (50., 5.)), 0);
    }

    #[test]
    fn operations_compose() {
        let operations = ops(vec![
            Operation::Rotate(Rotation::_90),
            Operation::MirrorHorizontally,
            Operation::Clip((1, 1, 2, 2)),
        ]);
        let (matrix, size) = operations_matrix(Some(&operations), (4., 3.));
        assert_eq!(size, (2., 2.));
        // (1, 0) -> rotate (0, 3) -> mirror (3, 3) -> clip (2, 2)
        assert_eq!(transformed(&matrix, (1., 0.)), (2, 2));

        let (matrix, size) = operations_matrix(None, (4., 3.));
        assert_eq!(size, (4., 3.));
        assert_eq!(transformed(&matrix, (1., 0.)), (1, 0));
    }

    /// Markup and pixels follow the same operations
    #[test]
    fn flatten_matches_replayed_pixels() {
        let operations = ops(vec![
            Operation::Rotate(Rotation::_270),
            Operation::Clip((2, 1, 6, 8)),
            Operation::MirrorVertically,
        ]);
        // Mark the original pixel (3, 5) of a 12×10 image both ways
        let mut original = RgbaImage::from_pixel(12, 10, image::Rgba([0, 0, 0, 255]));
        original.put_pixel(3, 5, image::Rgba([255, 255, 255, 255]));
        let replayed = crate::editing::render::apply_operations(original, &operations).unwrap();

        let mut drawn = RgbaImage::from_pixel(
            replayed.width(),
            replayed.height(),
            image::Rgba([0, 0, 0, 255]),
        );
        let (matrix, _) = operations_matrix(Some(&operations), (12., 10.));
        let dot = Shape::new(
            Kind::Rectangle {
                from: (3.25, 5.25),
                to: (3.75, 5.75),
            },
            Color::rgb(255, 255, 255),
            0.5,
        );
        flatten(&mut drawn, &[dot], matrix).unwrap();

        let brightest = |image: &RgbaImage| {
            image
                .enumerate_pixels()
                .max_by_key(|(_, _, p)| p.0[0])
                .map(|(x, y, _)| (x, y))
                .unwrap()
        };
        assert_eq!(brightest(&drawn), brightest(&replayed));
    }

    #[test]
    fn flatten_without_shapes_keeps_pixels() {
        let mut image = RgbaImage::from_fn(5, 5, |x, y| {
            image::Rgba([x as u8 * 40, y as u8 * 40, 7, 200])
        });
        let before = image.clone();
        flatten(&mut image, &[], cairo::Matrix::identity()).unwrap();
        assert_eq!(image, before);
    }

    #[test]
    fn flatten_roundtrip_keeps_untouched_pixels() {
        let mut image = RgbaImage::from_fn(40, 40, |x, y| {
            image::Rgba([x as u8 * 6, y as u8 * 6, 90, if x < 20 { 255 } else { 128 }])
        });
        let before = image.clone();
        let rect = Shape::new(
            Kind::Rectangle {
                from: (30., 30.),
                to: (35., 35.),
            },
            Color::rgb(255, 0, 0),
            1.,
        );
        flatten(&mut image, &[rect], cairo::Matrix::identity()).unwrap();

        // Far from the rectangle nothing changes, also with half alpha
        for (x, y) in [(2, 2), (10, 15), (25, 5)] {
            let a = before.get_pixel(x, y).0;
            let b = image.get_pixel(x, y).0;
            for c in 0..4 {
                assert!(a[c].abs_diff(b[c]) <= 1, "{x},{y}: {a:?} {b:?}");
            }
        }
    }

    #[test]
    fn flatten_draws_opaque_lines() {
        let mut image = RgbaImage::from_pixel(30, 30, image::Rgba([0, 0, 0, 0]));
        let stroke = Shape::new(
            Kind::Stroke {
                points: vec![(0., 15.), (30., 15.)],
                highlighter: false,
            },
            Color::rgb(0x35, 0x84, 0xe4),
            4.,
        );
        flatten(&mut image, &[stroke], cairo::Matrix::identity()).unwrap();
        assert_eq!(image.get_pixel(15, 15).0, [0x35, 0x84, 0xe4, 255]);
        assert_eq!(image.get_pixel(15, 2).0[3], 0);
    }

    #[test]
    fn flatten_keeps_faint_pixels_outside_shapes() {
        let mut image = RgbaImage::from_pixel(50, 50, image::Rgba([200, 10, 30, 2]));
        let dot = Shape::new(
            Kind::Rectangle {
                from: (40., 40.),
                to: (45., 45.),
            },
            Color::rgb(255, 0, 0),
            1.,
        );
        flatten(&mut image, &[dot], cairo::Matrix::identity()).unwrap();
        assert_eq!(image.get_pixel(5, 5).0, [200, 10, 30, 2]);
    }

    #[test]
    fn flatten_across_tiles() {
        let size = TILE_SIZE + 20;
        let mut image = RgbaImage::from_pixel(size, 10, image::Rgba([0, 0, 0, 255]));
        let stroke = Shape::new(
            Kind::Stroke {
                points: vec![(0., 5.), (size as f64, 5.)],
                highlighter: false,
            },
            Color::rgb(255, 255, 255),
            4.,
        );
        flatten(&mut image, &[stroke], cairo::Matrix::identity()).unwrap();
        for x in [10, TILE_SIZE - 1, TILE_SIZE, size - 10] {
            assert_eq!(image.get_pixel(x, 5).0, [255, 255, 255, 255], "{x}");
        }
    }

    #[test]
    fn hit_testing() {
        let line = Shape::new(
            Kind::Line {
                from: (0., 0.),
                to: (10., 0.),
            },
            Color::default(),
            2.,
        );
        assert!(line.hit((5., 1.5), 1.));
        assert!(!line.hit((5., 4.), 1.));

        let ellipse = Shape::new(
            Kind::Ellipse {
                from: (0., 0.),
                to: (20., 10.),
            },
            Color::default(),
            2.,
        );
        assert!(ellipse.hit((20., 5.), 1.));
        assert!(ellipse.hit((10., 0.), 1.));
        assert!(!ellipse.hit((10., 5.), 1.));

        let rect = Shape::new(
            Kind::Rectangle {
                from: (0., 0.),
                to: (10., 10.),
            },
            Color::default(),
            2.,
        );
        assert!(rect.hit((0., 5.), 1.));
        assert!(!rect.hit((5., 5.), 1.));

        let mut filled = rect.clone();
        filled.filled = true;
        assert!(filled.hit((5., 5.), 1.));
        let mut filled = ellipse.clone();
        filled.filled = true;
        assert!(filled.hit((10., 5.), 1.));
    }

    #[test]
    fn translate_moves_in_original_coordinates() {
        let mut shape = Shape::new(
            Kind::Line {
                from: (0., 0.),
                to: (1., 0.),
            },
            Color::default(),
            1.,
        );
        // Drawn in a view rotated by 90°
        shape.matrix = cairo::Matrix::new(0., -1., 1., 0., 0., 10.);
        shape.translate(3., 4.);
        assert_eq!(transformed(&shape.matrix, (0., 0.)), (3, 14));
    }

    #[test]
    fn ellipse_and_line_draw() {
        let mut image = RgbaImage::from_pixel(40, 40, image::Rgba([0, 0, 0, 255]));
        let ellipse = Shape::new(
            Kind::Ellipse {
                from: (5., 5.),
                to: (35., 35.),
            },
            Color::rgb(255, 255, 255),
            2.,
        );
        flatten(&mut image, &[ellipse], cairo::Matrix::identity()).unwrap();
        assert!(image.get_pixel(5, 20).0[0] > 200);
        assert_eq!(image.get_pixel(20, 20).0[0], 0);
    }

    #[test]
    fn filled_rectangle_covers_inside() {
        let mut image = RgbaImage::from_pixel(20, 20, image::Rgba([0, 0, 0, 255]));
        let mut rect = Shape::new(
            Kind::Rectangle {
                from: (4., 4.),
                to: (16., 16.),
            },
            Color::rgb(255, 0, 0),
            2.,
        );
        rect.filled = true;
        flatten(&mut image, &[rect], cairo::Matrix::identity()).unwrap();
        assert_eq!(image.get_pixel(10, 10).0, [255, 0, 0, 255]);
        assert_eq!(image.get_pixel(1, 1).0, [0, 0, 0, 255]);
    }

    #[test]
    fn highlighter_is_translucent() {
        let mut image = RgbaImage::from_pixel(30, 30, image::Rgba([255, 255, 255, 255]));
        let stroke = Shape::new(
            Kind::Stroke {
                points: vec![(0., 15.), (30., 15.)],
                highlighter: true,
            },
            Color::rgb(0, 0, 255),
            8.,
        );
        flatten(&mut image, &[stroke], cairo::Matrix::identity()).unwrap();
        let pixel = image.get_pixel(15, 15).0;
        // 40 % blue over white
        assert!(pixel[0].abs_diff(153) <= 2 && pixel[2] == 255, "{pixel:?}");
    }
}
