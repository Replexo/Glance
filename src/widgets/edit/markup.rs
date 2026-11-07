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

//! Bottom bar of the Markup view: tools, colour and undo/redo

use adw::prelude::*;
use adw::subclass::prelude::*;

use crate::deps::*;
use crate::editing::markup::{COLORS, Color, Tool};

const TOOLS: [(&str, Tool); 5] = [
    ("brush", Tool::Brush),
    ("highlighter", Tool::Highlighter),
    ("rectangle", Tool::Rectangle),
    ("arrow", Tool::Arrow),
    ("text", Tool::Text),
];

/// CSS class names of [`COLORS`], in the same order
const COLOR_NAMES: [&str; 5] = ["blue", "yellow", "red", "orange", "green"];

pub fn tool_from_name(name: &str) -> Option<Tool> {
    TOOLS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, tool)| *tool)
}

pub fn color_from_name(name: &str) -> Option<Color> {
    COLOR_NAMES
        .iter()
        .position(|n| *n == name)
        .map(|index| COLORS[index])
}

mod imp {
    use super::*;

    #[derive(Debug, Default, gtk::CompositeTemplate)]
    #[template(file = "markup.ui")]
    pub struct LpEditMarkup {
        #[template_child]
        color_indicator: TemplateChild<adw::Bin>,
        #[template_child]
        color_popover: TemplateChild<gtk::Popover>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for LpEditMarkup {
        const NAME: &'static str = "LpEditMarkup";
        type Type = super::LpEditMarkup;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            Self::bind_template(klass);
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for LpEditMarkup {}
    impl WidgetImpl for LpEditMarkup {}
    impl BinImpl for LpEditMarkup {}

    impl LpEditMarkup {
        pub(super) fn show_color(&self, name: &str) {
            for class in COLOR_NAMES {
                self.color_indicator.remove_css_class(class);
            }
            self.color_indicator.add_css_class(name);
            self.color_popover.popdown();
        }
    }
}

glib::wrapper! {
    pub struct LpEditMarkup(ObjectSubclass<imp::LpEditMarkup>)
    @extends gtk::Widget, adw::Bin,
    @implements gtk::Buildable, gtk::Accessible, gtk::ConstraintTarget;
}

impl LpEditMarkup {
    /// Shows the selected colour on the colour button and closes the popover
    pub fn show_color(&self, name: &str) {
        self.imp().show_color(name);
    }
}
