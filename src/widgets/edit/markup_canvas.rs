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

use std::cell::{Cell, OnceCell, RefCell};

use adw::prelude::*;
use adw::subclass::prelude::*;

use crate::deps::*;
use crate::editing::markup::{self, Color, Font, Kind, Shape, Tool};
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
        /// Font of new text
        pub(super) font: Cell<Font>,
        /// Handle of the selected shape that is being dragged, and the
        /// shape as it was when the drag started
        pub(super) handle: Cell<Option<Handle>>,
        pub(super) handle_start: RefCell<Option<Shape>>,
        /// Index of the text shape that is being typed into again
        pub(super) editing: Cell<Option<usize>>,
        /// Time and place of the last press, to notice double presses
        pub(super) last_press: Cell<Option<(u32, f64, f64)>>,
        pub(super) rotate_cursor: OnceCell<gdk::Cursor>,
        /// The text being typed is dragged by its outline
        pub(super) moving_text: Cell<bool>,
    }

    /// Handles around the selected shape
    #[derive(Debug, Clone, Copy, PartialEq)]
    pub enum Handle {
        /// Corner, clockwise from the top left; resizes around the opposite
        /// corner
        Corner(usize),
        /// Above the shape; rotates around its centre
        Rotate,
    }

    /// Text shape that is still being typed, and the caret as byte index.
    /// The text between `anchor` and `caret` is selected.
    #[derive(Debug)]
    pub struct Typing {
        pub(super) shape: Shape,
        pub(super) caret: usize,
        pub(super) anchor: usize,
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
            self.delete_selection();
            let caret = self.caret;
            if let Some(text) = self.text_mut() {
                text.insert_str(caret, string);
                self.caret += string.len();
            }
            self.anchor = self.caret;
        }

        /// Start and end of the selected text, if any
        pub(super) fn selection(&self) -> Option<(usize, usize)> {
            (self.anchor != self.caret)
                .then(|| (self.anchor.min(self.caret), self.anchor.max(self.caret)))
        }

        /// Removes the selected text, and whether there was any
        fn delete_selection(&mut self) -> bool {
            let Some((start, end)) = self.selection() else {
                return false;
            };
            if let Some(text) = self.text_mut() {
                text.replace_range(start..end, "");
            }
            self.caret = start;
            self.anchor = start;
            true
        }

        /// Puts the caret at `index`, without a selection
        pub(super) fn place(&mut self, index: usize) {
            self.caret = floor_boundary(self.text(), index);
            self.anchor = self.caret;
        }

        pub(super) fn select_all(&mut self) {
            self.anchor = 0;
            self.caret = self.text().len();
        }

        /// Selects the word around `index`
        pub(super) fn select_word(&mut self, index: usize) {
            let text = self.text();
            let index = floor_boundary(text, index);
            let is_word = |c: char| c.is_alphanumeric() || c == '_';
            let start = text[..index]
                .char_indices()
                .rev()
                .find(|(_, c)| !is_word(*c))
                .map_or(0, |(i, c)| i + c.len_utf8());
            let end = text[index..]
                .char_indices()
                .find(|(_, c)| !is_word(*c))
                .map_or(text.len(), |(i, _)| index + i);
            self.anchor = start;
            self.caret = end;
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
                text[..self.caret]
                    .char_indices()
                    .next_back()
                    .map(|(i, _)| i)
            }
        }

        pub(super) fn delete(&mut self, forward: bool) {
            if self.delete_selection() {
                return;
            }
            let Some(other) = self.neighbour(forward) else {
                return;
            };
            let (start, end) = (self.caret.min(other), self.caret.max(other));
            if let Some(text) = self.text_mut() {
                text.replace_range(start..end, "");
                self.caret = start;
            }
            self.anchor = self.caret;
        }

        pub(super) fn step(&mut self, forward: bool) {
            // Arrows end a selection at its edge
            if let Some((start, end)) = self.selection() {
                self.place(if forward { end } else { start });
                return;
            }
            if let Some(index) = self.neighbour(forward) {
                self.place(index);
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
            self.anchor = self.caret;
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
                move |gesture, x, y| obj.imp().drag_begin(x, y, gesture.current_event_time())
            ));
            drag.connect_drag_update(glib::clone!(
                #[weak]
                obj,
                move |gesture, x, y| {
                    let shift = gesture
                        .current_event_state()
                        .contains(gdk::ModifierType::SHIFT_MASK);
                    obj.imp().drag_update(x, y, shift)
                }
            ));
            drag.connect_drag_end(glib::clone!(
                #[weak]
                obj,
                move |_, _, _| obj.imp().drag_end()
            ));
            obj.add_controller(drag);

            // Resize, rotate and move cursors for the select tool
            let motion = gtk::EventControllerMotion::new();
            motion.connect_motion(glib::clone!(
                #[weak]
                obj,
                move |_, x, y| obj.imp().hover(x, y)
            ));
            obj.add_controller(motion);
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

            // Text being composed by the input method shows at the caret
            let typing = typing.as_ref().map(|typing| {
                let (preedit, _, _) = self.im_context.preedit_string();
                let mut shown = Typing {
                    shape: typing.shape.clone(),
                    caret: typing.caret,
                    anchor: typing.anchor,
                };
                if !preedit.is_empty() {
                    shown.insert(&preedit);
                }
                shown
            });
            // One layer, so an eraser stroke in progress clears the rest.
            // Text that is typed into again shows only as it is typed.
            let editing = self.editing.get().filter(|_| typing.is_some());
            let mut all: Vec<Shape> = shapes
                .iter()
                .enumerate()
                .filter(|(index, _)| Some(*index) != editing)
                .map(|(_, shape)| shape.clone())
                .collect();
            all.extend(current.iter().cloned());
            all.extend(typing.iter().map(|x| x.shape.clone()));
            markup::draw(&cr, &all);

            // Also outline the parts outside the picture
            cr.reset_clip();
            if let Some(shape) = selected {
                draw_outline(&cr, shape);
                if let Some(handles) = self.handle_points(shape) {
                    draw_handles(&cr, &handles);
                }
            }
            if let Some(typing) = typing.as_ref() {
                draw_outline(&cr, &typing.shape);
                draw_selection(&cr, typing);
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
                Tool::Eraser => "cell",
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
                Kind::Erase { .. } => markup::eraser_width(width, height),
                _ => markup::line_width(width, height),
            };
            let fillable = matches!(kind, Kind::Rectangle { .. } | Kind::Ellipse { .. });
            let mut shape = Shape::new(kind, self.color.get(), size * self.size_factor.get());
            shape.filled = fillable && self.filled.get();
            shape.font = self.font.get();
            // Shapes are stored relative to the original image
            let (matrix, _) = self.operations_matrix();
            if let Ok(inverse) = matrix.try_invert() {
                shape.matrix = inverse;
            }
            shape
        }

        /// Whether the press at `time` and `(x, y)` follows the last one
        /// closely enough to count as a double press
        fn double_press(&self, time: u32, x: f64, y: f64) -> bool {
            let settings = gtk::Settings::default();
            let (interval, distance) = settings.map_or((400, 5), |settings| {
                (
                    settings.gtk_double_click_time(),
                    settings.gtk_double_click_distance(),
                )
            });
            let last = self.last_press.replace(Some((time, x, y)));
            let double = last.is_some_and(|(last_time, last_x, last_y)| {
                time.wrapping_sub(last_time) <= interval as u32
                    && (x - last_x).hypot(y - last_y) <= distance as f64
            });
            if double {
                // A third press starts over
                self.last_press.set(None);
            }
            double
        }

        /// Topmost text shape under the widget point `(x, y)`
        fn text_at(&self, x: f64, y: f64) -> Option<usize> {
            let index = self.shape_at(x, y)?;
            let shapes = self.obj().edit_window().markup();
            matches!(shapes.get(index)?.kind, Kind::Text { .. }).then_some(index)
        }

        fn drag_begin(&self, x: f64, y: f64, time: u32) {
            self.drag_start.set((x, y));
            let double = self.double_press(time, x, y);

            let Some(point) = self.to_image(x, y) else {
                return;
            };

            let kind = match self.tool.get() {
                Tool::Select => {
                    self.moved.set(false);
                    // Double pressing text types into it again
                    if double && let Some(index) = self.text_at(x, y) {
                        let _ = self
                            .obj()
                            .activate_action("edit-markup.tool", Some(&"text".to_variant()));
                        self.edit_text(index, x, y);
                        return;
                    }
                    if let Some(handle) = self.handle_at(x, y) {
                        self.handle.set(Some(handle));
                        let shape = self
                            .selected
                            .get()
                            .and_then(|i| self.obj().edit_window().markup().get(i).cloned());
                        self.handle_start.replace(shape);
                        return;
                    }
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
                    self.text_click(x, y, point, double);
                    return;
                }
                Tool::Eraser => Kind::Erase {
                    points: vec![point],
                },
            };

            self.current.replace(Some(self.new_shape(kind)));
            self.obj().queue_draw();
        }

        fn drag_update(&self, offset_x: f64, offset_y: f64, shift: bool) {
            if self.moving_text.get() {
                self.move_typing(offset_x, offset_y);
                return;
            }
            if self.tool.get() == Tool::Select {
                let (start_x, start_y) = self.drag_start.get();
                match self.handle.get() {
                    Some(handle) => {
                        self.transform_selected(handle, (start_x + offset_x, start_y + offset_y))
                    }
                    None => self.move_selected(offset_x, offset_y),
                }
                return;
            }

            let (start_x, start_y) = self.drag_start.get();
            let Some(point) = self.to_image(start_x + offset_x, start_y + offset_y) else {
                return;
            };

            if let Some(shape) = self.current.borrow_mut().as_mut() {
                match &mut shape.kind {
                    Kind::Stroke { points, .. } | Kind::Erase { points } => points.push(point),
                    // Shift keeps lines at multiples of 45° and makes
                    // squares and circles
                    Kind::Line { from, to } | Kind::Arrow { from, to } => {
                        *to = if shift {
                            snap_angle(*from, point)
                        } else {
                            point
                        }
                    }
                    Kind::Rectangle { from, to } | Kind::Ellipse { from, to } => {
                        *to = if shift { square(*from, point) } else { point }
                    }
                    Kind::Text { .. } => {}
                }
            }
            self.obj().queue_draw();
        }

        fn drag_end(&self) {
            self.moving_text.set(false);
            self.handle.set(None);
            self.handle_start.take();
            let Some(shape) = self.current.take() else {
                return;
            };

            let empty = match &shape.kind {
                Kind::Line { from, to }
                | Kind::Rectangle { from, to }
                | Kind::Ellipse { from, to }
                | Kind::Arrow { from, to } => (from.0 - to.0).hypot(from.1 - to.1) < shape.size,
                // An eraser stroke over no markup changes nothing
                Kind::Erase { .. } => self.obj().edit_window().markup().is_empty(),
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

        /// Shape coordinates to widget coordinates
        fn shape_to_widget(&self, shape: &Shape) -> Option<cairo::Matrix> {
            let (x, y, scale) = image_transform(&self.image())?;
            let (matrix, _) = self.operations_matrix();
            let full = cairo::Matrix::multiply(&shape.matrix, &matrix);
            Some(cairo::Matrix::multiply(
                &full,
                &cairo::Matrix::new(scale, 0., 0., scale, x, y),
            ))
        }

        /// Corners of the outline in shape coordinates, clockwise from the
        /// top left, matching `draw_outline`
        fn outline_corners(shape: &Shape, to_widget: &cairo::Matrix) -> [(f64, f64); 4] {
            let (x0, y0, x1, y1) = shape.bounds();
            let pad = 6. / determinant(to_widget).abs().sqrt().max(f64::EPSILON);
            [
                (x0 - pad, y0 - pad),
                (x1 + pad, y0 - pad),
                (x1 + pad, y1 + pad),
                (x0 - pad, y1 + pad),
            ]
        }

        /// Corner handles and the rotate handle in widget coordinates
        pub(super) fn handle_points(&self, shape: &Shape) -> Option<[(f64, f64); 5]> {
            let to_widget = self.shape_to_widget(shape)?;
            let corners = Self::outline_corners(shape, &to_widget);
            let w = corners.map(|(x, y)| to_widget.transform_point(x, y));
            // The rotate handle sits above the middle of the top edge
            let top = ((w[0].0 + w[1].0) / 2., (w[0].1 + w[1].1) / 2.);
            let bottom = ((w[2].0 + w[3].0) / 2., (w[2].1 + w[3].1) / 2.);
            let (dx, dy) = (top.0 - bottom.0, top.1 - bottom.1);
            let length = dx.hypot(dy).max(f64::EPSILON);
            let rotate = (top.0 + dx / length * 24., top.1 + dy / length * 24.);
            Some([w[0], w[1], w[2], w[3], rotate])
        }

        fn handle_at(&self, x: f64, y: f64) -> Option<Handle> {
            let index = self.selected.get()?;
            let shape = self.obj().edit_window().markup().get(index).cloned()?;
            let points = self.handle_points(&shape)?;
            let (nearest, distance) = points
                .iter()
                .enumerate()
                .map(|(i, p)| (i, (p.0 - x).hypot(p.1 - y)))
                .min_by(|a, b| a.1.total_cmp(&b.1))?;
            (distance <= 10.).then_some(if nearest == 4 {
                Handle::Rotate
            } else {
                Handle::Corner(nearest)
            })
        }

        /// Resizes or rotates the selected shape so the handle follows the
        /// pointer at the widget point `pointer`
        fn transform_selected(&self, handle: Handle, pointer: (f64, f64)) {
            let Some(index) = self.selected.get() else {
                return;
            };
            let Some(start) = self.handle_start.borrow().clone() else {
                return;
            };
            let Some(to_widget) = self.shape_to_widget(&start) else {
                return;
            };
            let Ok(to_shape) = to_widget.try_invert() else {
                return;
            };
            let corners = Self::outline_corners(&start, &to_widget);
            let mut local = cairo::Matrix::identity();

            match handle {
                Handle::Rotate => {
                    let (x0, y0, x1, y1) = start.bounds();
                    let centre = ((x0 + x1) / 2., (y0 + y1) / 2.);
                    let c = to_widget.transform_point(centre.0, centre.1);
                    let (sx, sy) = self.drag_start.get();
                    let angle =
                        (pointer.1 - c.1).atan2(pointer.0 - c.0) - (sy - c.1).atan2(sx - c.0);
                    // A mirrored view turns the other way in shape coordinates
                    let sign = if determinant(&to_widget) < 0. {
                        -1.
                    } else {
                        1.
                    };
                    local.translate(centre.0, centre.1);
                    local.rotate(angle * sign);
                    local.translate(-centre.0, -centre.1);
                }
                Handle::Corner(corner) => {
                    let anchor = corners[(corner + 2) % 4];
                    let grabbed = corners[corner];
                    let p = to_shape.transform_point(pointer.0, pointer.1);
                    let (gx, gy) = (grabbed.0 - anchor.0, grabbed.1 - anchor.1);
                    let length = gx * gx + gy * gy;
                    if length <= 0. {
                        return;
                    }
                    let factor =
                        (((p.0 - anchor.0) * gx + (p.1 - anchor.1) * gy) / length).max(0.05);
                    local.translate(anchor.0, anchor.1);
                    local.scale(factor, factor);
                    local.translate(-anchor.0, -anchor.1);
                }
            }

            let mut shape = start;
            shape.matrix = cairo::Matrix::multiply(&local, &shape.matrix);
            self.obj()
                .edit_window()
                .replace_markup(index, shape, !self.moved.replace(true));
        }

        /// Shows what pressing at `(x, y)` would do with the select tool
        fn hover(&self, x: f64, y: f64) {
            let obj = self.obj();
            // The outline of the text being typed moves it
            if self.tool.get() == Tool::Text {
                if !self.moving_text.get() {
                    let on_border = self.on_typing_border(x, y);
                    obj.set_cursor_from_name(Some(if on_border { "move" } else { "text" }));
                }
                return;
            }
            // A drag keeps the cursor it started with
            if self.tool.get() != Tool::Select || self.handle.get().is_some() {
                return;
            }
            match self.handle_at(x, y) {
                Some(Handle::Rotate) => obj.set_cursor(Some(self.rotate_cursor())),
                Some(Handle::Corner(corner)) => {
                    obj.set_cursor_from_name(Some(self.corner_cursor(corner)))
                }
                None if self.shape_at(x, y).is_some() => obj.set_cursor_from_name(Some("move")),
                None => obj.set_cursor_from_name(Some("default")),
            }
        }

        /// Resize cursor that points from the centre of the selected shape
        /// towards `corner`
        fn corner_cursor(&self, corner: usize) -> &'static str {
            let points = self
                .selected
                .get()
                .and_then(|index| self.obj().edit_window().markup().get(index).cloned())
                .and_then(|shape| self.handle_points(&shape));
            let Some(points) = points else {
                return "nwse-resize";
            };
            let centre_x = points[..4].iter().map(|p| p.0).sum::<f64>() / 4.;
            let centre_y = points[..4].iter().map(|p| p.1).sum::<f64>() / 4.;
            let (dx, dy) = (points[corner].0 - centre_x, points[corner].1 - centre_y);
            // Direction in degrees, where opposite directions are the same
            let angle = dy.atan2(dx).to_degrees().rem_euclid(180.);
            match angle {
                a if (22.5..67.5).contains(&a) => "nwse-resize",
                a if (67.5..112.5).contains(&a) => "ns-resize",
                a if (112.5..157.5).contains(&a) => "nesw-resize",
                _ => "ew-resize",
            }
        }

        /// Circular arrow, since cursor themes have no rotate cursor
        fn rotate_cursor(&self) -> &gdk::Cursor {
            self.rotate_cursor.get_or_init(|| {
                let fallback = gdk::Cursor::from_name("grab", None);
                rotate_texture()
                    .map(|texture| gdk::Cursor::from_texture(&texture, 12, 12, fallback.as_ref()))
                    .or(fallback)
                    .unwrap_or_else(|| gdk::Cursor::from_name("default", None).unwrap())
            })
        }

        /// Widget point in the coordinates of `shape`, and the hit tolerance
        /// there
        fn shape_point(&self, shape: &Shape, x: f64, y: f64) -> Option<((f64, f64), f64)> {
            let (_, _, scale) = image_transform(&self.image())?;
            let point = self.to_image(x, y)?;
            let (matrix, _) = self.operations_matrix();
            let full = cairo::Matrix::multiply(&shape.matrix, &matrix);
            let local = full.try_invert().ok()?.transform_point(point.0, point.1);
            Some((local, 6. / scale))
        }

        /// Whether the widget point `(x, y)` is on the dashed outline of the
        /// text being typed
        fn on_typing_border(&self, x: f64, y: f64) -> bool {
            let typing = self.typing.borrow();
            let Some(shape) = typing.as_ref().map(|typing| &typing.shape) else {
                return false;
            };
            let Some(to_widget) = self.shape_to_widget(shape) else {
                return false;
            };
            let corners = Self::outline_corners(shape, &to_widget)
                .map(|(cx, cy)| to_widget.transform_point(cx, cy));
            (0..4).any(|i| segment_distance(corners[i], corners[(i + 1) % 4], (x, y)) <= 6.)
        }

        /// Moves the text being typed so it follows the drag
        fn move_typing(&self, offset_x: f64, offset_y: f64) {
            let Some((_, _, scale)) = image_transform(&self.image()) else {
                return;
            };
            let (previous_x, previous_y) = self
                .last_offset
                .replace(Some((offset_x, offset_y)))
                .unwrap_or((0., 0.));
            let (matrix, _) = self.operations_matrix();
            let Ok(inverse) = matrix.try_invert() else {
                return;
            };
            let (dx, dy) = inverse.transform_distance(
                (offset_x - previous_x) / scale,
                (offset_y - previous_y) / scale,
            );
            if let Some(typing) = self.typing.borrow_mut().as_mut() {
                typing.shape.translate(dx, dy);
            }
            self.text_changed();
        }

        /// Types into the text shape at `index` again, with the caret at
        /// the widget point `(x, y)`
        fn edit_text(&self, index: usize, x: f64, y: f64) {
            let obj = self.obj();
            let Some(shape) = obj.edit_window().markup().get(index).cloned() else {
                return;
            };
            let Kind::Text { position, text } = &shape.kind else {
                return;
            };
            let caret = self
                .shape_point(&shape, x, y)
                .map(|(local, _)| {
                    let local = (local.0 - position.0, local.1 - position.1);
                    markup::text_index_at(text, shape.size, shape.font, local)
                })
                .unwrap_or(text.len())
                .min(text.len());

            obj.grab_focus();
            self.selected.set(None);
            self.editing.set(Some(index));
            self.typing.replace(Some(Typing {
                shape,
                caret,
                anchor: caret,
            }));
            self.keys.set_im_context(Some(&self.im_context));
            self.im_context.focus_in();
            self.text_changed();
        }

        /// Starts typing at the clicked point, moves the caret within the
        /// text being typed, or finishes it on a click elsewhere. Clicking
        /// text types into it again.
        fn text_click(&self, x: f64, y: f64, point: (f64, f64), double: bool) {
            let obj = self.obj();
            obj.grab_focus();

            if self.on_typing_border(x, y) {
                self.moving_text.set(true);
                self.last_offset.set(None);
                return;
            }

            if self.typing.borrow().is_some() {
                // Pending composed text lands at the old caret, before the
                // click is measured against the text
                self.im_context.reset();
                let inside = self.typing_point(x, y).and_then(|(local, tolerance)| {
                    let typing = self.typing.borrow();
                    let typing = typing.as_ref()?;
                    let position = match &typing.shape.kind {
                        Kind::Text { position, .. } => *position,
                        _ => return None,
                    };
                    typing.shape.hit(local, tolerance).then(|| {
                        let local = (local.0 - position.0, local.1 - position.1);
                        markup::text_index_at(
                            typing.text(),
                            typing.shape.size,
                            typing.shape.font,
                            local,
                        )
                    })
                });

                match inside {
                    Some(index) => {
                        if let Some(typing) = self.typing.borrow_mut().as_mut() {
                            // Double pressing selects a word
                            if double {
                                typing.select_word(index);
                            } else {
                                typing.place(index);
                            }
                        }
                        self.text_changed();
                    }
                    None => {
                        self.finish_text();
                        if let Some(index) = self.text_at(x, y) {
                            self.edit_text(index, x, y);
                        }
                    }
                }
                return;
            }

            if let Some(index) = self.text_at(x, y) {
                self.edit_text(index, x, y);
                return;
            }

            // The click marks the middle of the first line
            let mut shape = self.new_shape(Kind::Text {
                position: point,
                text: String::new(),
            });
            let (_, line_height) = markup::text_size("", shape.size, shape.font);
            shape.kind = Kind::Text {
                position: (point.0, point.1 - line_height / 2.),
                text: String::new(),
            };

            self.typing.replace(Some(Typing {
                shape,
                caret: 0,
                anchor: 0,
            }));
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
            let typing = self.typing.borrow();
            self.shape_point(&typing.as_ref()?.shape, x, y)
        }

        /// Keys while typing
        fn typing_key(&self, key: gdk::Key, modifiers: gdk::ModifierType) -> glib::Propagation {
            let control = modifiers.contains(gdk::ModifierType::CONTROL_MASK);
            if control && matches!(key, gdk::Key::a | gdk::Key::A) {
                if let Some(typing) = self.typing.borrow_mut().as_mut() {
                    typing.select_all();
                }
                self.text_changed();
                return glib::Propagation::Stop;
            }
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
            if let Some(edit_window) = obj.try_edit_window() {
                edit_window.refresh_save_state();
            }

            let Some(typing) = self
                .typing
                .borrow()
                .as_ref()
                .map(|t| (t.shape.clone(), t.text().to_owned(), t.caret))
            else {
                return;
            };
            let (shape, text, caret) = typing;
            let Kind::Text { position, .. } = shape.kind else {
                return;
            };
            let Some((offset_x, offset_y, scale)) = image_transform(&self.image()) else {
                return;
            };
            let (top, bottom) = markup::text_caret(&text, shape.size, shape.font, caret);
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
            let ((x0, y0), (x1, y1)) = markup::text_caret(
                typing.text(),
                typing.shape.size,
                typing.shape.font,
                typing.caret,
            );

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
            let editing = self.editing.take();
            let obj = self.obj();
            let edit_window = obj.edit_window();
            if !typing.text().trim().is_empty() {
                let mut shape = typing.shape;
                if let Kind::Text { text, .. } = &mut shape.kind {
                    *text = text.trim_end().to_owned();
                }
                match editing {
                    // Text that stayed the same adds no undo step
                    Some(index) if edit_window.markup().get(index) == Some(&shape) => {}
                    Some(index) => edit_window.replace_markup(index, shape, true),
                    None => edit_window.add_markup(shape),
                }
            } else if let Some(index) = editing {
                // Deleting all of the text removes it
                edit_window.remove_markup(index);
            }
            obj.queue_draw();
        }

        fn cancel_text(&self) {
            self.editing.set(None);
            if self.typing.take().is_some() {
                self.im_context.reset();
                self.stop_input_method();
                let obj = self.obj();
                obj.queue_draw();
                if let Some(edit_window) = obj.try_edit_window() {
                    edit_window.refresh_save_state();
                }
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

/// Highlight behind the selected part of the text being typed, with `cr` in
/// image coordinates
fn draw_selection(cr: &cairo::Context, typing: &imp::Typing) {
    let Some((start, end)) = typing.selection() else {
        return;
    };
    let Kind::Text { position, .. } = typing.shape.kind else {
        return;
    };
    let shape = &typing.shape;
    cr.save().ok();
    cr.transform(shape.matrix);
    for (x, y, width, height) in
        markup::text_selection(typing.text(), shape.size, shape.font, start, end)
    {
        cr.rectangle(position.0 + x, position.1 + y, width, height);
    }
    cr.set_source_rgba(0.21, 0.52, 0.89, 0.4);
    let _ = cr.fill();
    cr.restore().ok();
}

/// `index` moved back to the nearest character boundary of `text`
fn floor_boundary(text: &str, index: usize) -> usize {
    let mut index = index.min(text.len());
    while !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

fn segment_distance(a: (f64, f64), b: (f64, f64), p: (f64, f64)) -> f64 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let length = dx * dx + dy * dy;
    if length == 0. {
        return (p.0 - a.0).hypot(p.1 - a.1);
    }
    let t = (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / length).clamp(0., 1.);
    (p.0 - a.0 - t * dx).hypot(p.1 - a.1 - t * dy)
}

fn determinant(matrix: &cairo::Matrix) -> f64 {
    matrix.xx() * matrix.yy() - matrix.yx() * matrix.xy()
}

/// Corner handles and the rotate handle, in widget coordinates
fn draw_handles(cr: &cairo::Context, points: &[(f64, f64); 5]) {
    cr.save().ok();
    cr.identity_matrix();
    cr.set_dash(&[], 0.);
    // Stem from the top edge to the rotate handle
    let top = (
        (points[0].0 + points[1].0) / 2.,
        (points[0].1 + points[1].1) / 2.,
    );
    cr.move_to(top.0, top.1);
    cr.line_to(points[4].0, points[4].1);
    cr.set_line_width(1.5);
    cr.set_source_rgb(1., 1., 1.);
    let _ = cr.stroke();
    for (i, (x, y)) in points.iter().enumerate() {
        let radius = if i == 4 { 6. } else { 5. };
        cr.arc(*x, *y, radius, 0., std::f64::consts::TAU);
        cr.set_source_rgb(1., 1., 1.);
        let _ = cr.fill_preserve();
        cr.set_line_width(1.5);
        cr.set_source_rgba(0., 0., 0., 0.6);
        let _ = cr.stroke();
    }
    cr.restore().ok();
}

/// End of a line from `from` towards `to`, turned to the nearest
/// multiple of 45°
fn snap_angle(from: (f64, f64), to: (f64, f64)) -> (f64, f64) {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let step = std::f64::consts::FRAC_PI_4;
    let angle = (dy.atan2(dx) / step).round() * step;
    let length = dx.hypot(dy);
    (from.0 + length * angle.cos(), from.1 + length * angle.sin())
}

/// Corner opposite `from` of the square over the box from `from` to
/// `to`, grown to its longer side
fn square(from: (f64, f64), to: (f64, f64)) -> (f64, f64) {
    let (dx, dy) = (to.0 - from.0, to.1 - from.1);
    let side = dx.abs().max(dy.abs());
    (from.0 + side * dx.signum(), from.1 + side * dy.signum())
}

/// Circular arrow for the rotate cursor, white with a dark edge
fn rotate_texture() -> Option<gdk::Texture> {
    let size = 24;
    let mut surface = cairo::ImageSurface::create(cairo::Format::ARgb32, size, size).ok()?;
    {
        let cr = cairo::Context::new(&surface).ok()?;
        let (c, radius) = (12., 6.5);
        let (start, end) = (-0.25 * std::f64::consts::PI, 1.3 * std::f64::consts::PI);
        // Head at the end of the arc, pointing along it
        let (tip_x, tip_y) = (c + radius * end.cos(), c + radius * end.sin());
        let (dx, dy) = (-end.sin(), end.cos());
        let head = |cr: &cairo::Context| {
            cr.move_to(tip_x + dx * 4.5, tip_y + dy * 4.5);
            cr.line_to(tip_x - dx * 1.5 - dy * 4., tip_y - dy * 1.5 + dx * 4.);
            cr.line_to(tip_x - dx * 1.5 + dy * 4., tip_y - dy * 1.5 - dx * 4.);
            cr.close_path();
        };
        cr.set_line_cap(cairo::LineCap::Round);
        cr.set_line_join(cairo::LineJoin::Round);
        for (width, shade) in [(4.5, 0.), (2., 1.)] {
            cr.set_source_rgb(shade, shade, shade);
            cr.set_line_width(width);
            cr.arc(c, c, radius, start, end - 0.2);
            cr.stroke().ok()?;
            head(&cr);
            if shade == 0. {
                cr.set_line_width(2.5);
                cr.stroke_preserve().ok()?;
            }
            cr.fill().ok()?;
        }
    }
    surface.flush();
    let stride = surface.stride() as usize;
    let data = surface.data().ok()?.to_vec();
    Some(
        gdk::MemoryTexture::new(
            size,
            size,
            gdk::MemoryFormat::B8g8r8a8Premultiplied,
            &glib::Bytes::from_owned(data),
            stride,
        )
        .upcast(),
    )
}

/// Offset and scale that map image coordinates to widget coordinates
fn image_transform(image: &LpImage) -> Option<(f64, f64, f64)> {
    let (width, _) = image.image_size();
    if width <= 0 {
        return None;
    }

    let scale = image.image_rendering_width() / width as f64;
    Some((
        image.image_rendering_x() - image.hadj().value(),
        image.image_rendering_y() - image.vadj().value(),
        scale,
    ))
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

    pub fn set_font(&self, font: Font) {
        let imp = self.imp();
        imp.font.set(font);
        if let Some(typing) = imp.typing.borrow_mut().as_mut() {
            typing.shape.font = font;
        }
        imp.keep_typing();
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

    /// Whether text with more than spaces is being typed
    pub fn has_typed_text(&self) -> bool {
        self.imp()
            .typing
            .borrow()
            .as_ref()
            .is_some_and(|typing| !typing.text().trim().is_empty())
    }

    /// Forgets the selected shape, for example after undo
    pub fn clear_selection(&self) {
        self.imp().selected.set(None);
        self.queue_draw();
    }
}
