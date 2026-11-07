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

//! Bottom bar of the Adjust view: one slider for the selected property

use std::cell::{Cell, OnceCell};

use adw::prelude::*;
use adw::subclass::prelude::*;

use crate::deps::*;
use crate::editing::adjust::{Adjustments, Property};
use crate::util::root::ParentWindow;

fn property_name(property: Property) -> &'static str {
    match property {
        Property::Brightness => "brightness",
        Property::Contrast => "contrast",
        Property::Highlights => "highlights",
        Property::Shadows => "shadows",
        Property::Saturation => "saturation",
    }
}

fn property_from_name(name: &str) -> Option<Property> {
    Property::ALL
        .into_iter()
        .find(|property| property_name(*property) == name)
}

mod imp {
    use super::*;

    #[derive(Debug, Default, gtk::CompositeTemplate)]
    #[template(file = "adjust.ui")]
    pub struct LpEditAdjust {
        #[template_child]
        pub(super) scale: TemplateChild<gtk::Scale>,
        #[template_child]
        pub(super) value_label: TemplateChild<gtk::Label>,

        pub(super) property: Cell<Option<Property>>,
        pub(super) scale_handler: OnceCell<glib::SignalHandlerId>,
        pub(super) automatic_action: OnceCell<gio::SimpleAction>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for LpEditAdjust {
        const NAME: &'static str = "LpEditAdjust";
        type Type = super::LpEditAdjust;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            Self::bind_template(klass);
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for LpEditAdjust {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();

            let actions = gio::SimpleActionGroup::new();

            let property_action = gio::SimpleAction::new_stateful(
                "property",
                Some(glib::VariantTy::STRING),
                &property_name(Property::Brightness).to_variant(),
            );
            property_action.connect_activate(glib::clone!(
                #[weak]
                obj,
                move |action, target| {
                    if let Some(name) = target.and_then(|x| x.get::<String>())
                        && let Some(property) = property_from_name(&name)
                    {
                        action.set_state(&name.to_variant());
                        obj.imp().select(property);
                    }
                }
            ));
            actions.add_action(&property_action);

            let automatic_action = gio::SimpleAction::new("automatic", None);
            automatic_action.connect_activate(glib::clone!(
                #[weak]
                obj,
                move |_, _| {
                    glib::spawn_future_local(async move { obj.imp().automatic().await });
                }
            ));
            actions.add_action(&automatic_action);
            self.automatic_action.set(automatic_action).unwrap();

            obj.insert_action_group("edit-adjust", Some(&actions));

            let handler = self.scale.connect_value_changed(glib::clone!(
                #[weak]
                obj,
                move |scale| obj.imp().scale_changed(scale.value())
            ));
            self.scale_handler.set(handler).unwrap();

            self.select(Property::Brightness);
        }
    }

    impl WidgetImpl for LpEditAdjust {}
    impl BinImpl for LpEditAdjust {}

    impl LpEditAdjust {
        fn select(&self, property: Property) {
            self.property.set(Some(property));
            self.sync();
        }

        /// Shows the stored value of the selected property
        pub(super) fn sync(&self) {
            let Some(property) = self.property.get() else {
                return;
            };

            let adjustments = self
                .obj()
                .try_edit_window()
                .map(|x| x.adjustments())
                .unwrap_or_default();

            let value = (adjustments.get(property) * 100.).round() as f64;
            let handler = self.scale_handler.get().unwrap();
            self.scale.block_signal(handler);
            self.scale.set_value(value);
            self.scale.unblock_signal(handler);
            self.update_label(value);
        }

        fn update_label(&self, value: f64) {
            let value = value.round() as i32;
            let label = if value > 0 {
                format!("+{value}")
            } else {
                // Minus sign instead of hyphen
                value.to_string().replace('-', "−")
            };
            self.value_label.set_label(&label);
        }

        fn scale_changed(&self, value: f64) {
            let Some(property) = self.property.get() else {
                return;
            };
            self.update_label(value);

            if let Some(edit_window) = self.obj().try_edit_window() {
                let mut adjustments = edit_window.adjustments();
                adjustments.set(property, value as f32 / 100.);
                edit_window.change_adjustments(adjustments);
            }
        }

        async fn automatic(&self) {
            let obj = self.obj();
            let Some(edit_window) = obj.try_edit_window() else {
                return;
            };
            let Some(file) = edit_window.original_image().file() else {
                return;
            };

            let action = self.automatic_action.get().unwrap();
            action.set_enabled(false);
            let sample = crate::editing::render::load_sample(file, 512).await;
            action.set_enabled(true);

            match sample {
                Ok(pixels) => {
                    let automatic = Adjustments::automatic(&pixels);
                    let mut adjustments = edit_window.adjustments();
                    adjustments.brightness = automatic.brightness;
                    adjustments.contrast = automatic.contrast;
                    adjustments.highlights = automatic.highlights;
                    adjustments.shadows = automatic.shadows;
                    edit_window.change_adjustments(adjustments);
                    self.sync();
                }
                Err(err) => tracing::warn!("Failed to sample image: {err}"),
            }
        }
    }
}

glib::wrapper! {
    pub struct LpEditAdjust(ObjectSubclass<imp::LpEditAdjust>)
    @extends gtk::Widget, adw::Bin,
    @implements gtk::Buildable, gtk::Accessible, gtk::ConstraintTarget;
}

impl LpEditAdjust {
    /// Updates the slider after the adjustments changed elsewhere
    pub fn sync(&self) {
        self.imp().sync();
    }
}
