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

//! Slider for values from -1 to 1 that fills from the middle
//!
//! With `centered` off it fills from the start instead, for plain amounts.
//!
//! Drawn without a frame so it can float over the picture: a thin track
//! with a dark halo, an accent fill between the middle and the knob, and a
//! white knob. Drag, click, arrow keys, Page Up/Down, Home/End and scrolling
//! change the value; double-click resets it to 0.

use std::cell::Cell;

use adw::prelude::*;
use adw::subclass::prelude::*;

use crate::deps::*;

const TRACK_HEIGHT: f32 = 6.;
const KNOB_SIZE: f32 = 20.;
const HALO: f32 = 3.;
const STEP: f64 = 0.01;
const PAGE: f64 = 0.1;

mod imp {
    use super::*;

    #[derive(Debug, Default, glib::Properties)]
    #[properties(wrapper_type = super::LpCenterSlider)]
    pub struct LpCenterSlider {
        /// Value from -1 to 1
        #[property(get, set = Self::set_value, explicit_notify, minimum = -1., maximum = 1.)]
        value: Cell<f64>,
        /// Fill from the middle and mark 0, otherwise fill from the start
        #[property(get, set, construct, default = true)]
        centered: Cell<bool>,
        drag_start: Cell<f64>,
        accent_handler: std::cell::RefCell<Option<glib::SignalHandlerId>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for LpCenterSlider {
        const NAME: &'static str = "LpCenterSlider";
        type Type = super::LpCenterSlider;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("centerslider");
            klass.set_accessible_role(gtk::AccessibleRole::Slider);
        }
    }

    #[glib::derived_properties]
    impl ObjectImpl for LpCenterSlider {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();

            obj.set_focusable(true);
            let handler = adw::StyleManager::default().connect_accent_color_notify(glib::clone!(
                #[weak]
                obj,
                move |_| obj.queue_draw()
            ));
            self.accent_handler.replace(Some(handler));
            obj.set_cursor_from_name(Some("pointer"));
            obj.update_property(&[
                gtk::accessible::Property::ValueMin(-100.),
                gtk::accessible::Property::ValueMax(100.),
                gtk::accessible::Property::ValueNow(0.),
            ]);

            let drag = gtk::GestureDrag::new();
            drag.connect_drag_begin(glib::clone!(
                #[weak]
                obj,
                move |_, x, _| {
                    obj.grab_focus();
                    obj.imp().drag_start.set(x);
                    obj.imp().set_from_x(x);
                }
            ));
            drag.connect_drag_update(glib::clone!(
                #[weak]
                obj,
                move |_, offset_x, _| {
                    let imp = obj.imp();
                    imp.set_from_x(imp.drag_start.get() + offset_x);
                }
            ));
            obj.add_controller(drag);

            let click = gtk::GestureClick::new();
            click.connect_pressed(glib::clone!(
                #[weak]
                obj,
                move |_, n_press, _, _| {
                    if n_press == 2 && obj.centered() {
                        obj.set_value(0.);
                    }
                }
            ));
            obj.add_controller(click);

            let keys = gtk::EventControllerKey::new();
            keys.connect_key_pressed(glib::clone!(
                #[weak]
                obj,
                #[upgrade_or]
                glib::Propagation::Proceed,
                move |_, key, _, _| {
                    let value = obj.value();
                    let new = match key {
                        gdk::Key::Left | gdk::Key::Down => value - STEP,
                        gdk::Key::Right | gdk::Key::Up => value + STEP,
                        gdk::Key::Page_Down => value - PAGE,
                        gdk::Key::Page_Up => value + PAGE,
                        gdk::Key::Home => -1.,
                        gdk::Key::End => 1.,
                        gdk::Key::BackSpace | gdk::Key::Delete if obj.centered() => 0.,
                        _ => return glib::Propagation::Proceed,
                    };
                    obj.set_value(new.clamp(-1., 1.));
                    glib::Propagation::Stop
                }
            ));
            obj.add_controller(keys);

            let scroll = gtk::EventControllerScroll::new(
                gtk::EventControllerScrollFlags::BOTH_AXES
                    | gtk::EventControllerScrollFlags::DISCRETE,
            );
            scroll.connect_scroll(glib::clone!(
                #[weak]
                obj,
                #[upgrade_or]
                glib::Propagation::Proceed,
                move |_, dx, dy| {
                    let delta = if dy != 0. { -dy } else { dx };
                    obj.set_value((obj.value() + delta * STEP * 5.).clamp(-1., 1.));
                    glib::Propagation::Stop
                }
            ));
            obj.add_controller(scroll);
        }

        fn dispose(&self) {
            self.disconnect_accent();
        }
    }

    impl LpCenterSlider {
        fn disconnect_accent(&self) {
            if let Some(handler) = self.accent_handler.take() {
                adw::StyleManager::default().disconnect(handler);
            }
        }
    }

    impl WidgetImpl for LpCenterSlider {
        fn state_flags_changed(&self, old_flags: &gtk::StateFlags) {
            self.parent_state_flags_changed(old_flags);
            // The focus ring is drawn by the widget itself
            self.obj().queue_draw();
        }

        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            match orientation {
                gtk::Orientation::Horizontal => {
                    // The width request is the size to use, not just a minimum
                    let width = self.obj().width_request();
                    if width > 0 {
                        (width.min(120), width, -1, -1)
                    } else {
                        (60, 300, -1, -1)
                    }
                }
                _ => {
                    let size = (KNOB_SIZE + 2. * HALO) as i32;
                    (size, size, -1, -1)
                }
            }
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let height = obj.height() as f32;
            let (start, end) = self.track_range();
            let middle = (start + end) / 2.;
            let knob = self.knob_x();
            let y = height / 2.;

            let halo = gdk::RGBA::new(0.06, 0.06, 0.07, 0.7);
            let track = gdk::RGBA::new(0.24, 0.24, 0.27, 1.);
            let accent = adw::StyleManager::default().accent_color_rgba();
            let white = gdk::RGBA::WHITE;

            let pill = |x: f32, y: f32, w: f32, h: f32| {
                gsk::RoundedRect::from_rect(graphene::Rect::new(x, y, w, h), h / 2.)
            };
            let fill = |rect: &gsk::RoundedRect, color: &gdk::RGBA| {
                snapshot.push_rounded_clip(rect);
                snapshot.append_color(color, rect.bounds());
                snapshot.pop();
            };

            // Halo, track and fill from the middle to the knob
            fill(
                &pill(
                    start - HALO,
                    y - TRACK_HEIGHT / 2. - HALO,
                    end - start + 2. * HALO,
                    TRACK_HEIGHT + 2. * HALO,
                ),
                &halo,
            );
            fill(
                &pill(start, y - TRACK_HEIGHT / 2., end - start, TRACK_HEIGHT),
                &track,
            );
            let origin = if obj.centered() { middle } else { start };
            let (from, to) = if knob < origin {
                (knob, origin)
            } else {
                (origin, knob)
            };
            snapshot.append_color(
                &accent,
                &graphene::Rect::new(from, y - TRACK_HEIGHT / 2., to - from, TRACK_HEIGHT),
            );
            if obj.centered() {
                // Mark for 0
                snapshot.append_color(
                    &gdk::RGBA::new(0.42, 0.42, 0.45, 1.),
                    &graphene::Rect::new(middle - 1., y - 6., 2., 12.),
                );
            }

            let knob_rect = |size: f32| pill(knob - size / 2., y - size / 2., size, size);
            fill(&knob_rect(KNOB_SIZE + 2. * HALO), &halo);
            fill(&knob_rect(KNOB_SIZE), &white);

            if obj.has_visible_focus() {
                let ring = knob_rect(KNOB_SIZE + 2. * HALO);
                snapshot.append_border(&ring, &[2.; 4], &[accent; 4]);
            }
        }
    }

    impl LpCenterSlider {
        fn set_value(&self, value: f64) {
            let value = (value.clamp(-1., 1.) * 100.).round() / 100.;
            if self.value.get() == value {
                return;
            }
            self.value.set(value);
            let obj = self.obj();
            obj.update_property(&[gtk::accessible::Property::ValueNow(value * 100.)]);
            obj.notify_value();
            obj.queue_draw();
        }

        /// Horizontal range the knob centre can move in
        fn track_range(&self) -> (f32, f32) {
            let width = self.obj().width() as f32;
            let inset = KNOB_SIZE / 2. + HALO;
            (inset, (width - inset).max(inset))
        }

        fn knob_x(&self) -> f32 {
            let (start, end) = self.track_range();
            start + (end - start) * ((self.value.get() as f32 + 1.) / 2.)
        }

        fn set_from_x(&self, x: f64) {
            let (start, end) = self.track_range();
            if end <= start {
                return;
            }
            let fraction = ((x as f32 - start) / (end - start)).clamp(0., 1.);
            self.obj().set_value(fraction as f64 * 2. - 1.);
        }
    }
}

glib::wrapper! {
    pub struct LpCenterSlider(ObjectSubclass<imp::LpCenterSlider>)
    @extends gtk::Widget,
    @implements gtk::Buildable, gtk::Accessible, gtk::ConstraintTarget;
}
