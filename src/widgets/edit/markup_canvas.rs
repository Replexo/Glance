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

//! Shows the markup on top of the image and lets the user draw new shapes

use std::cell::{Cell, RefCell};

use adw::prelude::*;
use adw::subclass::prelude::*;

use crate::deps::*;
use crate::editing::markup::{self, Color, Kind, Shape, Tool};
use crate::util::root::ParentWindow;
use crate::widgets::LpImage;
use crate::widgets::edit::LpEditCrop;

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub struct LpMarkupCanvas {
        pub(super) tool: Cell<Tool>,
        pub(super) color: Cell<Color>,
        /// Shape that is being drawn
        pub(super) current: RefCell<Option<Shape>>,
        pub(super) drag_start: Cell<(f64, f64)>,
        /// Text being typed onto the picture
        pub(super) typing: RefCell<Option<Typing>>,
        pub(super) im_context: gtk::IMMulticontext,
        /// Only filters keys through the input method while typing
        pub(super) keys: gtk::EventControllerKey,
        /// Factor for line width and font size of new shapes
        pub(super) size_factor: Cell<f64>,
        /// New rectangles and ellipses are filled
        pub(super) filled: Cell<bool>,
        /// Shape picked with the select tool, and whether it was moved in
        /// the current drag
        pub(super) selected: Cell<Option<usize>>,
        pub(super) moved: Cell<bool>,
        /// Drag offset at the last move
        pub(super) last_offset: Cell<Option<(f64, f64)>>,
    }

    /// Text shape that is still being typed, and the caret as byte index
    #[derive(Debug)]
    pub struct Typing {
        pub(super) shape: Shape,
        pub(super) caret: usize,
    }

    impl Typing {
        pub(super) fn text(&self) -> &str {
            match &self.shape.kind {
                Kind::Text { text, .. } => text,
                _ => "",
            }
        }

        fn text_mut(&mut self) -> Option<&mut String> {
            match &mut self.shape.kind {
                Kind::Text { text, .. } => Some(text),
                _ => None,
            }
        }

        pub(super) fn insert(&mut self, string: &str) {
            let caret = self.caret;
            if let Some(text) = self.text_mut() {
                text.insert_str(caret, string);
                self.caret += string.len();
            }
        }

        /// Byte index of the character boundary before or after the caret
        fn neighbour(&self, forward: bool) -> Option<usize> {
            let text = self.text();
            if forward {
                text[self.caret..]
                    .chars()
                    .next()
                    .map(|c| self.caret + c.len_utf8())
            } else {
                text[..self.caret].char_indices().next_back().map(|(i, _)| i)
            }
        }

        pub(super) fn delete(&mut self, forward: bool) {
            let Some(other) = self.neighbour(forward) else {
                return;
            };
            let (start, end) = (self.caret.min(other), self.caret.max(other));
            if let Some(text) = self.text_mut() {
                text.replace_range(start..end, "");
                self.caret = start;
            }
        }

        pub(super) fn step(&mut self, forward: bool) {
            if let Some(index) = self.neighbour(forward) {
                self.caret = index;
            }
        }

        /// Moves the caret to the start or end of its line
        pub(super) fn line_edge(&mut self, end: bool) {
            let text = self.text();
            self.caret = if end {
                text[self.caret..]
                    .find('\n')
                    .map_or(text.len(), |i| self.caret + i)
            } else {
                text[..self.caret].rfind('\n').map_or(0, |i| i + 1)
            };
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for LpMarkupCanvas {
        const NAME: &'static str = "LpMarkupCanvas";
        type Type = super::LpMarkupCanvas;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("lpmarkup");
        }
    }

    impl ObjectImpl for LpMarkupCanvas {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();

            obj.set_cursor_from_name(Some("crosshair"));
            obj.set_focusable(true);
            self.size_factor.set(1.);

            // Typed characters, including dead keys and compose sequences
            self.im_context.set_client_widget(Some(&*obj));
            self.im_context.connect_commit(glib::clone!(
                #[weak]
                obj,
                move |_, string| {
                    let imp = obj.imp();
                    if let Some(typing) = imp.typing.borrow_mut().as_mut() {
                        typing.insert(string);
                    }
                    imp.text_changed();
                }
            ));
            self.im_context.connect_preedit_changed(glib::clone!(
                #[weak]
                obj,
                move |_| obj.queue_draw()
            ));

            let focus = gtk::EventControllerFocus::new();
            focus.connect_enter(glib::clone!(
                #[weak]
                obj,
                move |_| {
                    let imp = obj.imp();
                    if imp.typing.borrow().is_some() {
                        imp.im_context.focus_in();
                    }
                }
            ));
            focus.connect_leave(glib::clone!(
                #[weak]
                obj,
                move |_| obj.imp().im_context.focus_out()
            ));
            obj.add_controller(focus);

            let keys = &self.keys;
            keys.connect_key_pressed(glib::clone!(
                #[weak]
                obj,
                #[upgrade_or]
                glib::Propagation::Proceed,
                move |_, key, _, modifiers| {
                    let imp = obj.imp();
                    if imp.typing.borrow().is_some() {
                        return imp.typing_key(key, modifiers);
                    }
                    match (key, imp.selected.get()) {
                        (gdk::Key::Delete | gdk::Key::BackSpace, Some(index)) => {
                            imp.selected.set(None);
                            obj.edit_window().remove_markup(index);
                            glib::Propagation::Stop
                        }
                        (gdk::Key::Escape, Some(_)) => {
                            imp.selected.set(None);
                            obj.queue_draw();
                            glib::Propagation::Stop
                        }
                        _ => glib::Propagation::Proceed,
                    }
                }
            ));
            obj.add_controller(keys.clone());

            let drag = gtk::GestureDrag::new();
            drag.connect_drag_begin(glib::clone!(
                #[weak]
                obj,
                move |_, x, y| obj.imp().drag_begin(x, y)
            ));
            drag.connect_drag_update(glib::clone!(
                #[weak]
                obj,
                move |_, x, y| obj.imp().drag_update(x, y)
            ));
            drag.connect_drag_end(glib::clone!(
                #[weak]
                obj,
                move |_, _, _| obj.imp().drag_end()
            ));
            obj.add_controller(drag);
        }

        fn dispose(&self) {
            self.im_context.set_client_widget(None::<&gtk::Widget>);
        }
    }

    impl WidgetImpl for LpMarkupCanvas {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let Some(edit_window) = obj.try_edit_window() else {
                return;
            };

            let shapes = edit_window.markup();
            let current = self.current.borrow();
            let typing = self.typing.borrow();
            if shapes.is_empty() && current.is_none() && typing.is_none() {
                return;
            }
            let selected = self.selected.get().and_then(|index| shapes.get(index));

            let image = self.image();
            let Some((x, y, scale)) = image_transform(&image) else {
                return;
            };

            let bounds = graphene::Rect::new(0., 0., obj.width() as f32, obj.height() as f32);
            let cr = snapshot.append_cairo(&bounds);
            cr.translate(x, y);
            cr.scale(scale, scale);

            // Only draw on the picture, like the saved image
            let (matrix, (width, height)) = self.operations_matrix();
            cr.rectangle(0., 0., width, height);
            cr.clip();
            cr.transform(matrix);

            markup::draw(&cr, &shapes);
            if let Some(shape) = current.as_ref() {
                markup::draw(&cr, std::slice::from_ref(shape));
            }
            // Text being composed by the input method shows at the caret
            let typing = typing.as_ref().map(|typing| {
                let (preedit, _, _) = self.im_context.preedit_string();
                let mut shown = Typing {
                    shape: typing.shape.clone(),
                    caret: typing.caret,
                };
                shown.insert(&preedit);
                shown
            });
            if let Some(typing) = typing.as_ref() {
                markup::draw(&cr, std::slice::from_ref(&typing.shape));
            }

            // Also outline the parts outside the picture
            cr.reset_clip();
            if let Some(shape) = selected {
                draw_outline(&cr, shape);
            }
            if let Some(typing) = typing.as_ref() {
                draw_outline(&cr, &typing.shape);
                self.draw_caret(&cr, typing);
            }
        }

        fn unmap(&self) {
            self.finish_text();
            self.parent_unmap();
        }
    }

    impl LpMarkupCanvas {
        pub(super) fn image(&self) -> LpImage {
            self.obj()
                .ancestor(LpEditCrop::static_type())
                .and_downcast::<LpEditCrop>()
                .unwrap()
                .image()
        }

        /// Matrix from original image coordinates to the current view of the
        /// image, and the image size in that view
        fn operations_matrix(&self) -> (cairo::Matrix, (f64, f64)) {
            let edit_window = self.obj().edit_window();
            let (width, height) = edit_window.original_image().image_size();
            markup::operations_matrix(
                edit_window.operations().as_deref(),
                (width as f64, height as f64),
            )
        }

        /// Widget to image coordinates
        fn to_image(&self, x: f64, y: f64) -> Option<(f64, f64)> {
            let (offset_x, offset_y, scale) = image_transform(&self.image())?;
            Some(((x - offset_x) / scale, (y - offset_y) / scale))
        }

        pub(super) fn restore_cursor(&self) {
            self.obj().set_cursor_from_name(Some(match self.tool.get() {
                Tool::Select => "default",
                Tool::Text => "text",
                _ => "crosshair",
            }));
        }

        /// Topmost shape under the widget point `(x, y)`
        fn shape_at(&self, x: f64, y: f64) -> Option<usize> {
            let (_, _, scale) = image_transform(&self.image())?;
            let point = self.to_image(x, y)?;
            let (matrix, _) = self.operations_matrix();
            let shapes = self.obj().edit_window().markup();

            shapes.iter().enumerate().rev().find_map(|(index, shape)| {
                let full = cairo::Matrix::multiply(&shape.matrix, &matrix);
                let local = full.try_invert().ok()?.transform_point(point.0, point.1);
                shape.hit(local, 6. / scale).then_some(index)
            })
        }

        fn new_shape(&self, kind: Kind) -> Shape {
            let (width, height) = self.image().image_size();
            let (width, height) = (width.max(1) as u32, height.max(1) as u32);
            let size = match &kind {
                Kind::Stroke {
                    highlighter: true, ..
                } => markup::highlighter_width(width, height),
                Kind::Text { .. } => markup::font_size(width, height),
                _ => markup::line_width(width, height),
            };
            let fillable = matches!(kind, Kind::Rectangle { .. } | Kind::Ellipse { .. });
            let mut shape = Shape::new(kind, self.color.get(), size * self.size_factor.get());
            shape.filled = fillable && self.filled.get();
            // Shapes are stored relative to the original image
            let (matrix, _) = self.operations_matrix();
            if let Ok(inverse) = matrix.try_invert() {
                shape.matrix = inverse;
            }
            shape
        }

        fn drag_begin(&self, x: f64, y: f64) {
            self.drag_start.set((x, y));

            let Some(point) = self.to_image(x, y) else {
                return;
            };

            let kind = match self.tool.get() {
                Tool::Select => {
                    self.selected.set(self.shape_at(x, y));
                    self.moved.set(false);
                    self.last_offset.set(None);
                    self.obj().grab_focus();
                    self.obj().queue_draw();
                    return;
                }
                Tool::Line => Kind::Line {
                    from: point,
                    to: point,
                },
                Tool::Ellipse => Kind::Ellipse {
                    from: point,
                    to: point,
                },
                Tool::Brush => Kind::Stroke {
                    points: vec![point],
                    highlighter: false,
                },
                Tool::Highlighter => Kind::Stroke {
                    points: vec![point],
                    highlighter: true,
                },
                Tool::Rectangle => Kind::Rectangle {
                    from: point,
                    to: point,
                },
                Tool::Arrow => Kind::Arrow {
                    from: point,
                    to: point,
                },
                Tool::Text => {
                    self.text_click(x, y, point);
                    return;
                }
            };

            self.current.replace(Some(self.new_shape(kind)));
            self.obj().queue_draw();
        }

        fn drag_update(&self, offset_x: f64, offset_y: f64) {
            if self.tool.get() == Tool::Select {
                self.move_selected(offset_x, offset_y);
                return;
            }

            let (start_x, start_y) = self.drag_start.get();
            let Some(point) = self.to_image(start_x + offset_x, start_y + offset_y) else {
                return;
            };

            if let Some(shape) = self.current.borrow_mut().as_mut() {
                match &mut shape.kind {
                    Kind::Stroke { points, .. } => points.push(point),
                    Kind::Line { to, .. }
                    | Kind::Rectangle { to, .. }
                    | Kind::Ellipse { to, .. }
                    | Kind::Arrow { to, .. } => *to = point,
                    Kind::Text { .. } => {}
                }
            }
            self.obj().queue_draw();
        }

        fn drag_end(&self) {
            let Some(shape) = self.current.take() else {
                return;
            };

            let empty = match &shape.kind {
                Kind::Line { from, to }
                | Kind::Rectangle { from, to }
                | Kind::Ellipse { from, to }
                | Kind::Arrow { from, to } => (from.0 - to.0).hypot(from.1 - to.1) < shape.size,
                _ => false,
            };

            if !empty {
                self.obj().edit_window().add_markup(shape);
            }
            self.obj().queue_draw();
        }

        /// Moves the selected shape so it follows the drag
        fn move_selected(&self, offset_x: f64, offset_y: f64) {
            let Some(index) = self.selected.get() else {
                return;
            };
            let Some((_, _, scale)) = image_transform(&self.image()) else {
                return;
            };
            let edit_window = self.obj().edit_window();
            let Some(mut shape) = edit_window.markup().get(index).cloned() else {
                return;
            };

            // Offset since the last update, in widget coordinates
            let (previous_x, previous_y) = self
                .last_offset
                .replace(Some((offset_x, offset_y)))
                .unwrap_or((0., 0.));
            let (delta_x, delta_y) = (offset_x - previous_x, offset_y - previous_y);
            // A click that barely moves is not a move
            if !self.moved.get() && offset_x.hypot(offset_y) < 2. {
                self.last_offset.set(None);
                return;
            }

            let (matrix, _) = self.operations_matrix();
            let Ok(inverse) = matrix.try_invert() else {
                return;
            };
            let (dx, dy) = inverse.transform_distance(delta_x / scale, delta_y / scale);
            shape.translate(dx, dy);

            edit_window.replace_markup(index, shape, !self.moved.replace(true));
        }

        /// Starts typing at the clicked point, moves the caret within the
        /// text being typed, or finishes it on a click elsewhere
        fn text_click(&self, x: f64, y: f64, point: (f64, f64)) {
            let obj = self.obj();
            obj.grab_focus();

            if self.typing.borrow().is_some() {
                let inside = self.typing_point(x, y).and_then(|(local, tolerance)| {
                    let typing = self.typing.borrow();
                    let typing = typing.as_ref()?;
                    let position = match &typing.shape.kind {
                        Kind::Text { position, .. } => *position,
                        _ => return None,
                    };
                    typing.shape.hit(local, tolerance).then(|| {
                        let local = (local.0 - position.0, local.1 - position.1);
                        markup::text_index_at(typing.text(), typing.shape.size, local)
                    })
                });

                match inside {
                    Some(index) => {
                        // Pending composed text lands at the old caret
                        self.im_context.reset();
                        if let Some(typing) = self.typing.borrow_mut().as_mut() {
                            typing.caret = index.min(typing.text().len());
                        }
                        self.text_changed();
                    }
                    None => self.finish_text(),
                }
                return;
            }

            // The click marks the middle of the first line
            let mut shape = self.new_shape(Kind::Text {
                position: point,
                text: String::new(),
            });
            let (_, line_height) = markup::text_size("", shape.size);
            shape.kind = Kind::Text {
                position: (point.0, point.1 - line_height / 2.),
                text: String::new(),
            };

            self.typing.replace(Some(Typing { shape, caret: 0 }));
            self.keys.set_im_context(Some(&self.im_context));
            self.im_context.focus_in();
            self.text_changed();
        }

        /// Keeps typing after the colour or size was changed in the pill
        pub(super) fn keep_typing(&self) {
            if self.typing.borrow().is_some() {
                self.text_changed();
                self.obj().grab_focus();
            }
        }

        /// Widget point in the coordinates of the text being typed, and the
        /// hit tolerance there
        fn typing_point(&self, x: f64, y: f64) -> Option<((f64, f64), f64)> {
            let (_, _, scale) = image_transform(&self.image())?;
            let point = self.to_image(x, y)?;
            let (matrix, _) = self.operations_matrix();
            let typing = self.typing.borrow();
            let full = cairo::Matrix::multiply(&typing.as_ref()?.shape.matrix, &matrix);
            let local = full.try_invert().ok()?.transform_point(point.0, point.1);
            Some((local, 6. / scale))
        }

        /// Keys while typing
        fn typing_key(&self, key: gdk::Key, modifiers: gdk::ModifierType) -> glib::Propagation {
            // Shortcuts like undo still reach the window
            if modifiers.intersects(gdk::ModifierType::CONTROL_MASK | gdk::ModifierType::ALT_MASK) {
                return glib::Propagation::Proceed;
            }

            let enter = matches!(
                key,
                gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::ISO_Enter
            );
            if key == gdk::Key::Escape {
                self.cancel_text();
                return glib::Propagation::Stop;
            }
            if enter && !modifiers.contains(gdk::ModifierType::SHIFT_MASK) {
                self.finish_text();
                return glib::Propagation::Stop;
            }

            if let Some(typing) = self.typing.borrow_mut().as_mut() {
                match key {
                    _ if enter => typing.insert("\n"),
                    gdk::Key::BackSpace => typing.delete(false),
                    gdk::Key::Delete | gdk::Key::KP_Delete => typing.delete(true),
                    gdk::Key::Left | gdk::Key::KP_Left => typing.step(false),
                    gdk::Key::Right | gdk::Key::KP_Right => typing.step(true),
                    gdk::Key::Home | gdk::Key::KP_Home => typing.line_edge(false),
                    gdk::Key::End | gdk::Key::KP_End => typing.line_edge(true),
                    // Keep the focus on the text
                    gdk::Key::Tab
                    | gdk::Key::ISO_Left_Tab
                    | gdk::Key::Up
                    | gdk::Key::Down
                    | gdk::Key::KP_Up
                    | gdk::Key::KP_Down => return glib::Propagation::Stop,
                    _ => return glib::Propagation::Proceed,
                }
            }
            self.text_changed();
            glib::Propagation::Stop
        }

        /// Redraws the text being typed and tells the input method where
        /// the caret is
        pub(super) fn text_changed(&self) {
            let obj = self.obj();
            obj.queue_draw();

            let Some(typing) = self.typing.borrow().as_ref().map(|t| {
                (t.shape.clone(), t.text().to_owned(), t.caret)
            }) else {
                return;
            };
            let (shape, text, caret) = typing;
            let Kind::Text { position, .. } = shape.kind else {
                return;
            };
            let Some((offset_x, offset_y, scale)) = image_transform(&self.image()) else {
                return;
            };
            let (top, bottom) = markup::text_caret(&text, shape.size, caret);
            let (matrix, _) = self.operations_matrix();
            let full = cairo::Matrix::multiply(&shape.matrix, &matrix);
            let to_widget = |(x, y): (f64, f64)| {
                let (x, y) = full.transform_point(position.0 + x, position.1 + y);
                (offset_x + x * scale, offset_y + y * scale)
            };
            let (x0, y0) = to_widget(top);
            let (x1, y1) = to_widget(bottom);
            self.im_context.set_cursor_location(&gdk::Rectangle::new(
                x0.min(x1) as i32,
                y0.min(y1) as i32,
                (x1 - x0).abs().max(1.) as i32,
                (y1 - y0).abs().max(1.) as i32,
            ));
        }

        /// Caret of the text being typed, with `cr` in image coordinates
        fn draw_caret(&self, cr: &cairo::Context, typing: &Typing) {
            let Kind::Text { position, .. } = typing.shape.kind else {
                return;
            };
            let ((x0, y0), (x1, y1)) =
                markup::text_caret(typing.text(), typing.shape.size, typing.caret);

            cr.save().ok();
            cr.transform(typing.shape.matrix);
            cr.move_to(position.0 + x0, position.1 + y0);
            cr.line_to(position.0 + x1, position.1 + y1);
            cr.restore().ok();
            cr.save().ok();
            cr.identity_matrix();
            cr.set_dash(&[], 0.);
            cr.set_line_width(2.);
            let color = typing.shape.color;
            cr.set_source_rgb(
                color.r as f64 / 255.,
                color.g as f64 / 255.,
                color.b as f64 / 255.,
            );
            let _ = cr.stroke();
            cr.restore().ok();
        }

        /// Adds the text being typed as a shape, if it has any
        pub(super) fn finish_text(&self) {
            if self.typing.borrow().is_none() {
                return;
            }
            // Adds what the input method still composes
            self.im_context.reset();
            self.stop_input_method();
            let Some(typing) = self.typing.take() else {
                return;
            };
            let obj = self.obj();
            if !typing.text().trim().is_empty() {
                let mut shape = typing.shape;
                if let Kind::Text { text, .. } = &mut shape.kind {
                    *text = text.trim_end().to_owned();
                }
                obj.edit_window().add_markup(shape);
            }
            obj.queue_draw();
        }

        fn cancel_text(&self) {
            if self.typing.take().is_some() {
                self.im_context.reset();
                self.stop_input_method();
                self.obj().queue_draw();
            }
        }

        fn stop_input_method(&self) {
            self.im_context.focus_out();
            self.keys.set_im_context(None::<&gtk::IMContext>);
        }
    }
}

/// Dashed outline around `shape`, in screen pixels, with `cr` in image
/// coordinates
fn draw_outline(cr: &cairo::Context, shape: &Shape) {
    let (x0, y0, x1, y1) = shape.bounds();
    cr.save().ok();
    cr.transform(shape.matrix);
    // Works under rotation, where x and y swap
    let (pad_x, pad_y) = cr.device_to_user_distance(6., 0.).unwrap_or((6., 0.));
    let pad = pad_x.hypot(pad_y);
    cr.rectangle(x0 - pad, y0 - pad, x1 - x0 + 2. * pad, y1 - y0 + 2. * pad);
    cr.identity_matrix();
    cr.set_line_width(1.5);
    cr.set_dash(&[5., 4.], 0.);
    cr.set_source_rgb(1., 1., 1.);
    let _ = cr.stroke_preserve();
    cr.set_dash(&[5., 4.], 5.);
    cr.set_source_rgb(0., 0., 0.);
    let _ = cr.stroke();
    cr.restore().ok();
}

/// Offset and scale that map image coordinates to widget coordinates
fn image_transform(image: &LpImage) -> Option<(f64, f64, f64)> {
    let (width, _) = image.image_size();
    if width <= 0 {
        return None;
    }

    let scale = image.image_rendering_width() / width as f64;
    Some((image.image_rendering_x(), image.image_rendering_y(), scale))
}

glib::wrapper! {
    pub struct LpMarkupCanvas(ObjectSubclass<imp::LpMarkupCanvas>)
    @extends gtk::Widget,
    @implements gtk::Buildable, gtk::Accessible, gtk::ConstraintTarget;
}

impl LpMarkupCanvas {
    pub fn set_tool(&self, tool: Tool) {
        let imp = self.imp();
        imp.finish_text();
        imp.tool.set(tool);
        if tool != Tool::Select && imp.selected.take().is_some() {
            self.queue_draw();
        }
        imp.restore_cursor();
    }

    pub fn set_color(&self, color: Color) {
        let imp = self.imp();
        imp.color.set(color);
        if let Some(typing) = imp.typing.borrow_mut().as_mut() {
            typing.shape.color = color;
        }
        imp.keep_typing();
    }
}

impl LpMarkupCanvas {
    pub fn set_filled(&self, filled: bool) {
        self.imp().filled.set(filled);
    }

    pub fn set_size_factor(&self, factor: f64) {
        let imp = self.imp();
        let old = imp.size_factor.replace(factor);
        if let Some(typing) = imp.typing.borrow_mut().as_mut() {
            typing.shape.size *= factor / old;
        }
        imp.keep_typing();
    }

    /// Adds the text being typed, before the markup is used elsewhere
    pub fn finish_text(&self) {
        self.imp().finish_text();
    }

    pub fn is_typing(&self) -> bool {
        self.imp().typing.borrow().is_some()
    }

    /// Forgets the selected shape, for example after undo
    pub fn clear_selection(&self) {
        self.imp().selected.set(None);
        self.queue_draw();
    }
}
