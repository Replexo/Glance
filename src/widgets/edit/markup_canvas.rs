// Copyright (c) 2026 madarra16
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
use crate::util::gettext::*;
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
        pub(super) text_popover: RefCell<Option<gtk::Popover>>,
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

            let keys = gtk::EventControllerKey::new();
            keys.connect_key_pressed(glib::clone!(
                #[weak]
                obj,
                #[upgrade_or]
                glib::Propagation::Proceed,
                move |_, key, _, _| {
                    let imp = obj.imp();
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
            obj.add_controller(keys);

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
            if let Some(popover) = self.text_popover.take() {
                popover.unparent();
            }
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
            if shapes.is_empty() && current.is_none() {
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

            // Dashed outline around the selected shape, in screen pixels
            if let Some(shape) = selected {
                let (x0, y0, x1, y1) = shape.bounds();
                // Also outline the parts outside the picture
                cr.reset_clip();
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
            }
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            self.parent_size_allocate(width, height, baseline);
            if let Some(popover) = self.text_popover.borrow().as_ref() {
                popover.present();
            }
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
            let tool = self.tool.get();
            self.obj()
                .set_cursor_from_name(Some(if tool == Tool::Select {
                    "default"
                } else {
                    "crosshair"
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
                    self.ask_text(x, y, point);
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

        /// Shows an entry at the clicked point and adds the text on Enter
        fn ask_text(&self, x: f64, y: f64, position: (f64, f64)) {
            let obj = self.obj();

            if let Some(popover) = self.text_popover.take() {
                popover.unparent();
            }

            let entry = gtk::Entry::builder()
                .placeholder_text(gettext("Text"))
                .width_chars(20)
                .build();
            let popover = gtk::Popover::builder()
                .child(&entry)
                .pointing_to(&gdk::Rectangle::new(x as i32, y as i32, 1, 1))
                .position(gtk::PositionType::Bottom)
                .build();
            popover.set_parent(&*obj);

            entry.connect_activate(glib::clone!(
                #[weak]
                obj,
                #[weak]
                popover,
                move |entry| {
                    let text = entry.text().trim_end().to_string();
                    if !text.is_empty() {
                        let shape = obj.imp().new_shape(Kind::Text { position, text });
                        obj.edit_window().add_markup(shape);
                    }
                    popover.popdown();
                }
            ));

            popover.connect_closed(glib::clone!(
                #[weak]
                obj,
                move |popover| {
                    let imp = obj.imp();
                    if imp.text_popover.borrow().as_ref() == Some(popover) {
                        imp.text_popover.take();
                        let popover = popover.clone();
                        // Unparenting inside the signal handler is not allowed
                        glib::idle_add_local_once(move || popover.unparent());
                    }
                }
            ));

            self.text_popover.replace(Some(popover.clone()));
            popover.popup();
            entry.grab_focus();
        }
    }
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
        imp.tool.set(tool);
        if tool != Tool::Select && imp.selected.take().is_some() {
            self.queue_draw();
        }
        imp.restore_cursor();
    }

    pub fn set_color(&self, color: Color) {
        self.imp().color.set(color);
    }
}

impl LpMarkupCanvas {
    pub fn set_filled(&self, filled: bool) {
        self.imp().filled.set(filled);
    }

    pub fn set_size_factor(&self, factor: f64) {
        self.imp().size_factor.set(factor);
    }

    /// Forgets the selected shape, for example after undo
    pub fn clear_selection(&self) {
        self.imp().selected.set(None);
        self.queue_draw();
    }
}
