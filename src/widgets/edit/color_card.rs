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

//! Small colour picker: saturation/value square, hue bar and hex entry

use std::cell::Cell;
use std::sync::LazyLock;

use adw::prelude::*;
use adw::subclass::prelude::*;
use glib::subclass::Signal;

use crate::deps::*;
use crate::editing::markup::Color;
use crate::util::gettext::*;

const SQUARE_WIDTH: i32 = 200;
const SQUARE_HEIGHT: i32 = 110;
const HUE_HEIGHT: i32 = 12;

/// HSV with hue in degrees, saturation and value from 0 to 1
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Hsv {
    pub h: f64,
    pub s: f64,
    pub v: f64,
}

impl Hsv {
    pub fn from_color(color: Color) -> Self {
        let (r, g, b) = (
            color.r as f64 / 255.,
            color.g as f64 / 255.,
            color.b as f64 / 255.,
        );
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let delta = max - min;

        let h = if delta == 0. {
            0.
        } else if max == r {
            60. * ((g - b) / delta).rem_euclid(6.)
        } else if max == g {
            60. * ((b - r) / delta + 2.)
        } else {
            60. * ((r - g) / delta + 4.)
        };
        let s = if max == 0. { 0. } else { delta / max };

        Self { h, s, v: max }
    }

    pub fn rgb(&self) -> (f64, f64, f64) {
        let c = self.v * self.s;
        let h = self.h.rem_euclid(360.) / 60.;
        let x = c * (1. - (h % 2. - 1.).abs());
        let (r, g, b) = match h as u32 {
            0 => (c, x, 0.),
            1 => (x, c, 0.),
            2 => (0., c, x),
            3 => (0., x, c),
            4 => (x, 0., c),
            _ => (c, 0., x),
        };
        let m = self.v - c;
        (r + m, g + m, b + m)
    }

    pub fn to_color(self) -> Color {
        let (r, g, b) = self.rgb();
        let channel = |x: f64| (x.clamp(0., 1.) * 255.).round() as u8;
        Color::rgb(channel(r), channel(g), channel(b))
    }
}

pub fn color_to_hex(color: Color) -> String {
    format!("#{:02X}{:02X}{:02X}", color.r, color.g, color.b)
}

pub fn color_from_hex(text: &str) -> Option<Color> {
    let text = text.trim();
    let hex = text.strip_prefix('#').unwrap_or(text);
    if !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let value = |range: std::ops::Range<usize>| u8::from_str_radix(hex.get(range)?, 16).ok();
    match hex.len() {
        6 => Some(Color::rgb(value(0..2)?, value(2..4)?, value(4..6)?)),
        3 => {
            let short = |i: usize| value(i..i + 1).map(|x| x * 17);
            Some(Color::rgb(short(0)?, short(1)?, short(2)?))
        }
        _ => None,
    }
}

mod imp {
    use super::*;

    #[derive(Debug)]
    pub struct LpColorCard {
        pub(super) square: gtk::DrawingArea,
        pub(super) hue: gtk::DrawingArea,
        pub(super) entry: gtk::Entry,
        pub(super) hsv: Cell<Hsv>,
        /// Ignore entry changes made by the card itself
        pub(super) updating: Cell<bool>,
    }

    impl Default for LpColorCard {
        fn default() -> Self {
            Self {
                square: gtk::DrawingArea::new(),
                hue: gtk::DrawingArea::new(),
                entry: gtk::Entry::new(),
                hsv: Cell::new(Hsv {
                    h: 285.,
                    s: 0.6,
                    v: 0.68,
                }),
                updating: Cell::new(false),
            }
        }
    }

    #[glib::object_subclass]
    impl ObjectSubclass for LpColorCard {
        const NAME: &'static str = "LpColorCard";
        type Type = super::LpColorCard;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("colorcard");
        }
    }

    impl ObjectImpl for LpColorCard {
        fn signals() -> &'static [Signal] {
            static SIGNALS: LazyLock<Vec<Signal>> =
                LazyLock::new(|| vec![Signal::builder("color-changed").build()]);
            SIGNALS.as_ref()
        }

        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();

            obj.set_orientation(gtk::Orientation::Vertical);
            obj.set_spacing(12);

            self.square.set_content_width(SQUARE_WIDTH);
            self.square.set_content_height(SQUARE_HEIGHT);
            self.square.set_cursor_from_name(Some("crosshair"));
            self.square.set_draw_func(glib::clone!(
                #[weak]
                obj,
                move |_, cr, width, height| obj.imp().draw_square(cr, width, height)
            ));
            self.add_drag(&self.square.clone(), |imp, x, y, width, height| {
                let mut hsv = imp.hsv.get();
                hsv.s = (x / width).clamp(0., 1.);
                hsv.v = 1. - (y / height).clamp(0., 1.);
                imp.set_hsv(hsv);
            });
            obj.append(&self.square);

            self.hue.set_content_width(SQUARE_WIDTH);
            self.hue.set_content_height(HUE_HEIGHT + 6);
            self.hue.set_draw_func(glib::clone!(
                #[weak]
                obj,
                move |_, cr, width, height| obj.imp().draw_hue(cr, width, height)
            ));
            self.add_drag(&self.hue.clone(), |imp, x, _, width, height| {
                // Same range as the drawn knob
                let knob = (height / 2. - 1.).max(1.);
                let fraction = (x - knob) / (width - 2. * knob).max(1.);
                let mut hsv = imp.hsv.get();
                hsv.h = fraction.clamp(0., 0.999) * 360.;
                imp.set_hsv(hsv);
            });
            obj.append(&self.hue);

            self.entry.set_hexpand(true);
            self.entry.set_max_length(7);
            self.entry.set_width_chars(8);
            self.entry.add_css_class("monospace");
            self.entry
                .update_property(&[gtk::accessible::Property::Label(&gettext("Hex Color"))]);
            self.entry.connect_changed(glib::clone!(
                #[weak]
                obj,
                move |entry| {
                    let imp = obj.imp();
                    if imp.updating.get() {
                        return;
                    }
                    if let Some(color) = color_from_hex(&entry.text()) {
                        imp.set_hsv_without_entry(Hsv::from_color(color));
                    }
                }
            ));
            obj.append(&self.entry);

            self.update_entry();
        }
    }

    impl WidgetImpl for LpColorCard {}
    impl BoxImpl for LpColorCard {}

    impl LpColorCard {
        /// Calls `f` with the pointer position while clicking or dragging
        fn add_drag(
            &self,
            area: &gtk::DrawingArea,
            f: impl Fn(&Self, f64, f64, f64, f64) + Clone + 'static,
        ) {
            let obj = self.obj();
            let drag = gtk::GestureDrag::new();
            let begin = f.clone();
            drag.connect_drag_begin(glib::clone!(
                #[weak]
                obj,
                #[weak]
                area,
                move |_, x, y| begin(obj.imp(), x, y, area.width() as f64, area.height() as f64)
            ));
            drag.connect_drag_update(glib::clone!(
                #[weak]
                obj,
                #[weak]
                area,
                move |gesture, dx, dy| {
                    if let Some((x, y)) = gesture.start_point() {
                        f(
                            obj.imp(),
                            x + dx,
                            y + dy,
                            area.width() as f64,
                            area.height() as f64,
                        );
                    }
                }
            ));
            area.add_controller(drag);
        }

        pub(super) fn set_hsv(&self, hsv: Hsv) {
            self.set_hsv_without_entry(hsv);
            self.update_entry();
        }

        fn set_hsv_without_entry(&self, hsv: Hsv) {
            self.hsv.set(hsv);
            self.square.queue_draw();
            self.hue.queue_draw();
            self.obj().emit_by_name::<()>("color-changed", &[]);
        }

        fn update_entry(&self) {
            self.updating.set(true);
            self.entry
                .set_text(&color_to_hex(self.hsv.get().to_color()));
            self.updating.set(false);
        }

        fn draw_square(&self, cr: &cairo::Context, width: i32, height: i32) {
            let (w, h) = (width as f64, height as f64);
            let hsv = self.hsv.get();
            let radius = 10.;

            rounded_rect(cr, 0., 0., w, h, radius);
            cr.clip();

            let (r, g, b) = Hsv {
                s: 1.,
                v: 1.,
                ..hsv
            }
            .rgb();
            let across = cairo::LinearGradient::new(0., 0., w, 0.);
            across.add_color_stop_rgb(0., 1., 1., 1.);
            across.add_color_stop_rgb(1., r, g, b);
            let _ = cr.set_source(&across);
            let _ = cr.paint();

            let down = cairo::LinearGradient::new(0., 0., 0., h);
            down.add_color_stop_rgba(0., 0., 0., 0., 0.);
            down.add_color_stop_rgba(1., 0., 0., 0., 1.);
            let _ = cr.set_source(&down);
            let _ = cr.paint();

            let (x, y) = (hsv.s * w, (1. - hsv.v) * h);
            cr.arc(x, y, 7., 0., std::f64::consts::TAU);
            cr.set_line_width(2.);
            cr.set_source_rgb(1., 1., 1.);
            let _ = cr.stroke();
        }

        fn draw_hue(&self, cr: &cairo::Context, width: i32, height: i32) {
            let (w, h) = (width as f64, height as f64);
            let bar = HUE_HEIGHT as f64;
            let top = (h - bar) / 2.;

            let gradient = cairo::LinearGradient::new(0., 0., w, 0.);
            for i in 0..=6 {
                let (r, g, b) = Hsv {
                    h: i as f64 * 60.,
                    s: 1.,
                    v: 1.,
                }
                .rgb();
                gradient.add_color_stop_rgb(i as f64 / 6., r, g, b);
            }
            rounded_rect(cr, 0., top, w, bar, bar / 2.);
            let _ = cr.set_source(&gradient);
            let _ = cr.fill();

            let knob = (h / 2. - 1.).max(1.);
            let x = knob + self.hsv.get().h / 360. * (w - 2. * knob).max(0.);
            cr.arc(x, h / 2., knob, 0., std::f64::consts::TAU);
            cr.set_source_rgb(1., 1., 1.);
            let _ = cr.fill();
        }
    }
}

fn rounded_rect(cr: &cairo::Context, x: f64, y: f64, w: f64, h: f64, r: f64) {
    use std::f64::consts::{FRAC_PI_2, PI};
    cr.new_sub_path();
    cr.arc(x + w - r, y + r, r, -FRAC_PI_2, 0.);
    cr.arc(x + w - r, y + h - r, r, 0., FRAC_PI_2);
    cr.arc(x + r, y + h - r, r, FRAC_PI_2, PI);
    cr.arc(x + r, y + r, r, PI, PI + FRAC_PI_2);
    cr.close_path();
}

glib::wrapper! {
    pub struct LpColorCard(ObjectSubclass<imp::LpColorCard>)
    @extends gtk::Widget, gtk::Box,
    @implements gtk::Buildable, gtk::Accessible, gtk::ConstraintTarget, gtk::Orientable;
}

impl LpColorCard {
    pub fn color(&self) -> Color {
        self.imp().hsv.get().to_color()
    }

    pub fn set_color(&self, color: Color) {
        self.imp().set_hsv(Hsv::from_color(color));
    }

    pub fn connect_color_changed<F: Fn(&Self) + 'static>(&self, f: F) -> glib::SignalHandlerId {
        self.connect_closure(
            "color-changed",
            false,
            glib::closure_local!(move |obj: &Self| f(obj)),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hsv_roundtrip() {
        for color in [
            Color::rgb(0x91, 0x41, 0xac),
            Color::rgb(255, 0, 0),
            Color::rgb(0, 0, 0),
            Color::rgb(255, 255, 255),
            Color::rgb(0x33, 0xd1, 0x7a),
            Color::rgb(12, 200, 250),
        ] {
            assert_eq!(Hsv::from_color(color).to_color(), color, "{color:?}");
        }
    }

    #[test]
    fn hex() {
        let color = Color::rgb(0x91, 0x41, 0xac);
        assert_eq!(color_to_hex(color), "#9141AC");
        assert_eq!(color_from_hex("#9141ac"), Some(color));
        assert_eq!(color_from_hex(" 9141AC "), Some(color));
        assert_eq!(color_from_hex("#f0a"), Some(Color::rgb(255, 0, 170)));
        assert_eq!(color_from_hex("#12345"), None);
        assert_eq!(color_from_hex("zzzzzz"), None);
        assert_eq!(color_from_hex("+1+2+3"), None);
        assert_eq!(color_from_hex("##abc"), None);
    }
}
