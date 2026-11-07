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
            let mut shape = Shape::new(kind, self.color.get(), size);
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
            let (start_x, start_y) = self.drag_start.get();
            let Some(point) = self.to_image(start_x + offset_x, start_y + offset_y) else {
                return;
            };

            if let Some(shape) = self.current.borrow_mut().as_mut() {
                match &mut shape.kind {
                    Kind::Stroke { points, .. } => points.push(point),
                    Kind::Rectangle { to, .. } | Kind::Arrow { to, .. } => *to = point,
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
                Kind::Rectangle { from, to } | Kind::Arrow { from, to } => {
                    (from.0 - to.0).hypot(from.1 - to.1) < shape.size
                }
                _ => false,
            };

            if !empty {
                self.obj().edit_window().add_markup(shape);
            }
            self.obj().queue_draw();
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
        self.imp().tool.set(tool);
    }

    pub fn set_color(&self, color: Color) {
        self.imp().color.set(color);
    }
}
