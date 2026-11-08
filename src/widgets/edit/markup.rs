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

//! Bottom bar of the Markup view: tools, colour and undo/redo

use std::cell::{Cell, RefCell};

use adw::prelude::*;
use adw::subclass::prelude::*;

use crate::deps::*;
use crate::editing::markup::{COLORS, Color, Font, Tool};
use crate::widgets::edit::LpColorCard;

const TOOLS: [(&str, Tool); 9] = [
    ("select", Tool::Select),
    ("brush", Tool::Brush),
    ("highlighter", Tool::Highlighter),
    ("line", Tool::Line),
    ("arrow", Tool::Arrow),
    ("rectangle", Tool::Rectangle),
    ("ellipse", Tool::Ellipse),
    ("text", Tool::Text),
    ("eraser", Tool::Eraser),
];

const FONTS: [(&str, Font); 4] = [
    ("sans", Font::Sans),
    ("serif", Font::Serif),
    ("mono", Font::Mono),
    ("hand", Font::Hand),
];

pub fn font_from_name(name: &str) -> Option<Font> {
    FONTS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, font)| *font)
}

/// CSS class names of [`COLORS`], in the same order
const COLOR_NAMES: [&str; 5] = ["blue", "yellow", "red", "orange", "green"];

/// Line width factors for the width buttons
const WIDTHS: [(&str, f64); 4] = [("s", 0.5), ("m", 1.), ("l", 1.75), ("xl", 3.)];

pub fn tool_from_name(name: &str) -> Option<Tool> {
    TOOLS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, tool)| *tool)
}

pub fn width_from_name(name: &str) -> Option<f64> {
    WIDTHS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, factor)| *factor)
}

mod imp {
    use super::*;

    #[derive(Debug, Default, gtk::CompositeTemplate)]
    #[template(file = "markup.ui")]
    pub struct LpEditMarkup {
        #[template_child]
        custom_swatch: TemplateChild<gtk::DrawingArea>,
        #[template_child]
        color_popover: TemplateChild<gtk::Popover>,
        #[template_child]
        color_card: TemplateChild<LpColorCard>,
        #[template_child]
        swatch_blue: TemplateChild<gtk::Button>,
        #[template_child]
        swatch_yellow: TemplateChild<gtk::Button>,
        #[template_child]
        swatch_red: TemplateChild<gtk::Button>,
        #[template_child]
        swatch_orange: TemplateChild<gtk::Button>,
        #[template_child]
        swatch_green: TemplateChild<gtk::Button>,
        #[template_child]
        pub(super) options_pill: TemplateChild<gtk::Widget>,
        #[template_child]
        pub(super) color_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub(super) fill_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub(super) fill_outline: TemplateChild<gtk::Button>,
        #[template_child]
        pub(super) fill_filled: TemplateChild<gtk::Button>,
        #[template_child]
        pub(super) width_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub(super) width_separator: TemplateChild<gtk::Widget>,
        #[template_child]
        pub(super) width_thin: TemplateChild<gtk::Widget>,
        #[template_child]
        pub(super) text_box: TemplateChild<gtk::Box>,

        /// Colour picked in the colour card
        pub(super) custom: Cell<Option<Color>>,
        pub(super) selected: RefCell<String>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for LpEditMarkup {
        const NAME: &'static str = "LpEditMarkup";
        type Type = super::LpEditMarkup;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            LpColorCard::static_type();
            Self::bind_template(klass);
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for LpEditMarkup {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();

            // Keep a gap to the pill below
            self.color_popover.set_offset(0, -10);

            self.custom_swatch.set_draw_func(glib::clone!(
                #[weak]
                obj,
                move |_, cr, width, height| obj.imp().draw_custom(cr, width, height)
            ));

            // Opening the card starts from and selects the custom colour
            self.color_popover.connect_show(glib::clone!(
                #[weak]
                obj,
                move |_| {
                    let imp = obj.imp();
                    match imp.custom.get() {
                        // Setting the same colour again would lose the hue
                        // of greys
                        Some(color) if color != imp.color_card.color() => {
                            imp.color_card.set_color(color)
                        }
                        Some(_) => {}
                        None => imp.custom.set(Some(imp.color_card.color())),
                    }
                    imp.select_custom();
                }
            ));
            self.color_card.connect_color_changed(glib::clone!(
                #[weak]
                obj,
                move |card| {
                    let imp = obj.imp();
                    imp.custom.set(Some(card.color()));
                    imp.select_custom();
                }
            ));
        }
    }

    impl WidgetImpl for LpEditMarkup {}
    impl BinImpl for LpEditMarkup {}

    impl LpEditMarkup {
        fn swatches(&self) -> [(&str, &gtk::Button); 5] {
            [
                ("blue", &*self.swatch_blue),
                ("yellow", &*self.swatch_yellow),
                ("red", &*self.swatch_red),
                ("orange", &*self.swatch_orange),
                ("green", &*self.swatch_green),
            ]
        }

        pub(super) fn select_custom(&self) {
            self.custom_swatch.queue_draw();
            let _ = self
                .obj()
                .activate_action("edit-markup.color", Some(&"custom".to_variant()));
        }

        pub(super) fn show_color(&self, name: &str) {
            self.selected.replace(name.to_owned());
            for (swatch_name, button) in self.swatches() {
                if swatch_name == name {
                    button.add_css_class("selected");
                } else {
                    button.remove_css_class("selected");
                }
            }
            self.custom_swatch.queue_draw();
        }

        /// Dashed ring with a plus while empty; after picking, one solid
        /// ring around the colour, white while the colour is in use
        fn draw_custom(&self, cr: &cairo::Context, width: i32, height: i32) {
            let (cx, cy) = (width as f64 / 2., height as f64 / 2.);
            let radius = cx.min(cy) - 1.5;
            let tau = std::f64::consts::TAU;
            let foreground = WidgetExt::color(&*self.obj());

            match self.custom.get() {
                None => {
                    cr.set_dash(&[3., 3.], 0.);
                    cr.set_line_width(1.5);
                    cr.set_source_rgba(0.42, 0.42, 0.45, 1.);
                    cr.arc(cx, cy, radius - 1., 0., tau);
                    let _ = cr.stroke();
                    cr.set_dash(&[], 0.);
                    cr.set_source_rgba(
                        foreground.red() as f64,
                        foreground.green() as f64,
                        foreground.blue() as f64,
                        0.8,
                    );
                }
                Some(color) => {
                    let in_use = self.selected.borrow().as_str() == "custom";
                    cr.set_line_width(2.);
                    if in_use {
                        cr.set_source_rgb(1., 1., 1.);
                    } else {
                        cr.set_source_rgba(0.42, 0.42, 0.45, 1.);
                    }
                    cr.arc(cx, cy, radius - 1., 0., tau);
                    let _ = cr.stroke();
                    cr.set_source_rgb(
                        color.r as f64 / 255.,
                        color.g as f64 / 255.,
                        color.b as f64 / 255.,
                    );
                    cr.arc(cx, cy, radius - 4., 0., tau);
                    let _ = cr.fill();
                    // Plus that stays readable on light colours
                    let luma =
                        0.2126 * color.r as f64 + 0.7152 * color.g as f64 + 0.0722 * color.b as f64;
                    let shade = if luma > 170. { 0. } else { 1. };
                    cr.set_source_rgb(shade, shade, shade);
                }
            }

            // Plus, centred on whole pixels
            let arm = 4.5;
            let (px, py) = (cx.round(), cy.round());
            cr.set_line_width(2.);
            cr.set_line_cap(cairo::LineCap::Round);
            cr.move_to(px - arm, py);
            cr.line_to(px + arm, py);
            cr.move_to(px, py - arm);
            cr.line_to(px, py + arm);
            let _ = cr.stroke();
        }
    }
}

glib::wrapper! {
    pub struct LpEditMarkup(ObjectSubclass<imp::LpEditMarkup>)
    @extends gtk::Widget, adw::Bin,
    @implements gtk::Buildable, gtk::Accessible, gtk::ConstraintTarget;
}

impl LpEditMarkup {
    /// Colour for a swatch name, including the picked custom colour
    pub fn color(&self, name: &str) -> Option<Color> {
        if name == "custom" {
            return self.imp().custom.get();
        }
        COLOR_NAMES
            .iter()
            .position(|n| *n == name)
            .map(|index| COLORS[index])
    }

    /// Marks the swatch of the selected colour
    pub fn show_color(&self, name: &str) {
        self.imp().show_color(name);
    }

    /// Shows only the options that `tool` uses
    pub fn set_tool(&self, tool: Tool) {
        let imp = self.imp();
        imp.options_pill.set_visible(tool != Tool::Select);
        imp.color_box
            .set_visible(!matches!(tool, Tool::Select | Tool::Eraser));
        imp.fill_box
            .set_visible(matches!(tool, Tool::Rectangle | Tool::Ellipse));
        let (outline, filled) = if tool == Tool::Ellipse {
            ("glance-circle-symbolic", "glance-circle-filled-symbolic")
        } else {
            ("glance-outline-symbolic", "glance-filled-symbolic")
        };
        imp.fill_outline.set_icon_name(outline);
        imp.fill_filled.set_icon_name(filled);
        imp.width_box.set_visible(tool != Tool::Text);
        // The eraser has no other options to separate from
        imp.width_separator.set_visible(tool != Tool::Eraser);
        // The highlighter is wide already
        imp.width_thin.set_visible(tool != Tool::Highlighter);
        imp.text_box.set_visible(tool == Tool::Text);
    }
}
