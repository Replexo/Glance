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

//! Controls of the Adjust view: the selected property's name and slider
//! above a pill of property buttons

use std::cell::{Cell, OnceCell};

use adw::prelude::*;
use adw::subclass::prelude::*;

use crate::deps::*;
use crate::editing::adjust::{Adjustments, Property};
use crate::util::gettext::*;
use crate::util::root::ParentWindow;
use crate::widgets::edit::LpCenterSlider;

fn property_label(property: Property) -> String {
    match property {
        Property::Brightness => gettext("Brightness"),
        Property::Contrast => gettext("Contrast"),
        Property::Highlights => gettext("Highlights"),
        Property::Shadows => gettext("Shadows"),
        Property::Saturation => gettext("Saturation"),
    }
}

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
        pub(super) slider: TemplateChild<LpCenterSlider>,
        #[template_child]
        name_label: TemplateChild<gtk::Label>,
        #[template_child]
        dot_brightness: TemplateChild<adw::Bin>,
        #[template_child]
        dot_contrast: TemplateChild<adw::Bin>,
        #[template_child]
        dot_highlights: TemplateChild<adw::Bin>,
        #[template_child]
        dot_shadows: TemplateChild<adw::Bin>,
        #[template_child]
        dot_saturation: TemplateChild<adw::Bin>,

        pub(super) property: Cell<Option<Property>>,
        pub(super) slider_handler: OnceCell<glib::SignalHandlerId>,
        pub(super) automatic_action: OnceCell<gio::SimpleAction>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for LpEditAdjust {
        const NAME: &'static str = "LpEditAdjust";
        type Type = super::LpEditAdjust;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            LpCenterSlider::static_type();
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

            let handler = self.slider.connect_value_notify(glib::clone!(
                #[weak]
                obj,
                move |slider| obj.imp().slider_changed(slider.value())
            ));
            self.slider_handler.set(handler).unwrap();

            self.select(Property::Brightness);
        }
    }

    impl WidgetImpl for LpEditAdjust {}
    impl BinImpl for LpEditAdjust {}

    impl LpEditAdjust {
        fn select(&self, property: Property) {
            self.property.set(Some(property));
            self.sync();
            if let Some(edit_window) = self.obj().try_edit_window() {
                edit_window.end_coalescing();
            }
        }

        /// Shows the stored value of the selected property and which
        /// properties are changed
        pub(super) fn sync(&self) {
            let Some(property) = self.property.get() else {
                return;
            };

            let adjustments = self
                .obj()
                .try_edit_window()
                .map(|x| x.adjustments())
                .unwrap_or_default();

            self.name_label.set_label(&property_label(property));

            let handler = self.slider_handler.get().unwrap();
            self.slider.block_signal(handler);
            self.slider.set_value(adjustments.get(property) as f64);
            self.slider.unblock_signal(handler);

            for (dot, property) in [
                (&*self.dot_brightness, Property::Brightness),
                (&*self.dot_contrast, Property::Contrast),
                (&*self.dot_highlights, Property::Highlights),
                (&*self.dot_shadows, Property::Shadows),
                (&*self.dot_saturation, Property::Saturation),
            ] {
                dot.set_visible(adjustments.get(property) != 0.);
            }
        }

        fn slider_changed(&self, value: f64) {
            let Some(property) = self.property.get() else {
                return;
            };

            if let Some(edit_window) = self.obj().try_edit_window() {
                let mut adjustments = edit_window.adjustments();
                adjustments.set(property, value as f32);
                edit_window.change_adjustments(adjustments, Some(property));
            }
            self.sync();
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
                    edit_window.change_adjustments(adjustments, None);
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
