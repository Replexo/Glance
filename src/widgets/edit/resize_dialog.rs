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

//! Dialog to make the image smaller, keeping its aspect ratio

use std::cell::Cell;
use std::sync::LazyLock;

use adw::prelude::*;
use adw::subclass::prelude::*;
use glib::subclass::Signal;

use crate::deps::*;
use crate::editing::render::scaled_size;

mod imp {
    use super::*;

    #[derive(Debug, Default, gtk::CompositeTemplate)]
    #[template(file = "resize_dialog.ui")]
    pub struct LpResizeDialog {
        #[template_child]
        width_adjustment: TemplateChild<gtk::Adjustment>,
        #[template_child]
        height_adjustment: TemplateChild<gtk::Adjustment>,
        #[template_child]
        scale_adjustment: TemplateChild<gtk::Adjustment>,

        /// Image size before resizing
        pub(super) size: Cell<(u32, u32)>,
        pub(super) scale: Cell<f64>,
        updating: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for LpResizeDialog {
        const NAME: &'static str = "LpResizeDialog";
        type Type = super::LpResizeDialog;
        type ParentType = adw::Dialog;

        fn class_init(klass: &mut Self::Class) {
            Self::bind_template(klass);

            klass.install_action("resize.apply", None, |obj, _, _| {
                obj.emit_by_name::<()>("scale-chosen", &[&obj.imp().scale.get()]);
                obj.close();
            });
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for LpResizeDialog {
        fn signals() -> &'static [Signal] {
            static SIGNALS: LazyLock<Vec<Signal>> = LazyLock::new(|| {
                vec![
                    Signal::builder("scale-chosen")
                        .param_types([f64::static_type()])
                        .build(),
                ]
            });
            SIGNALS.as_ref()
        }

        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();

            self.width_adjustment.connect_value_changed(glib::clone!(
                #[weak]
                obj,
                move |adjustment| {
                    let imp = obj.imp();
                    let width = imp.size.get().0.max(1) as f64;
                    imp.set_scale(adjustment.value() / width);
                }
            ));

            self.height_adjustment.connect_value_changed(glib::clone!(
                #[weak]
                obj,
                move |adjustment| {
                    let imp = obj.imp();
                    let height = imp.size.get().1.max(1) as f64;
                    imp.set_scale(adjustment.value() / height);
                }
            ));

            self.scale_adjustment.connect_value_changed(glib::clone!(
                #[weak]
                obj,
                move |adjustment| obj.imp().set_scale(adjustment.value() / 100.)
            ));
        }
    }

    impl WidgetImpl for LpResizeDialog {}
    impl AdwDialogImpl for LpResizeDialog {}

    impl LpResizeDialog {
        pub(super) fn setup(&self, size: (u32, u32), scale: f64) {
            self.size.set(size);
            self.width_adjustment.set_upper(size.0.max(1) as f64);
            self.height_adjustment.set_upper(size.1.max(1) as f64);
            self.set_scale(scale);
        }

        /// Shows `scale` in all fields, without feeding the changes back
        pub(super) fn set_scale(&self, scale: f64) {
            if self.updating.get() {
                return;
            }
            self.updating.set(true);

            let scale = scale.clamp(0., 1.);
            let (width, height) = scaled_size(self.size.get(), scale);
            self.scale.set(scale);
            self.width_adjustment.set_value(width as f64);
            self.height_adjustment.set_value(height as f64);
            self.scale_adjustment.set_value(scale * 100.);

            self.updating.set(false);
        }
    }
}

glib::wrapper! {
    pub struct LpResizeDialog(ObjectSubclass<imp::LpResizeDialog>)
    @extends gtk::Widget, adw::Dialog,
    @implements gtk::Buildable, gtk::Accessible, gtk::ConstraintTarget, gtk::ShortcutManager;
}

impl LpResizeDialog {
    /// Dialog for an image of `size` that is currently scaled by `scale`
    pub fn new(size: (u32, u32), scale: f64) -> Self {
        let obj: Self = glib::Object::new();
        obj.imp().setup(size, scale);
        obj
    }

    /// Called with the chosen scale factor when Resize is clicked
    pub fn connect_scale_chosen<F: Fn(&Self, f64) + 'static>(&self, f: F) -> glib::SignalHandlerId {
        self.connect_closure(
            "scale-chosen",
            false,
            glib::closure_local!(move |obj: &Self, scale: f64| f(obj, scale)),
        )
    }
}
