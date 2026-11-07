// Copyright (c) 2024-2026 Sophie Herold
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

//! Shows an resizable cropping selection

use std::cell::{OnceCell, RefCell};
use std::sync::Arc;

use adw::prelude::*;
use adw::subclass::prelude::*;
use adw::{glib, gtk};
use glycin::{Operation, Operations};

use crate::deps::*;
use crate::editing::preview::EditingError;
use crate::util::ErrorType;
use crate::util::gettext::*;
use crate::util::root::ParentWindow;
use crate::widgets::LpImage;
use crate::widgets::edit::{LpEditAdjust, LpEditCropSelection};

/// Aspect ratio modes that can be selected
#[derive(Debug, Clone, Copy, Default, glib::Enum, glib::Variant)]
#[enum_type(name = "LpAspectRatio")]
pub enum LpAspectRatio {
    #[default]
    Free,
    Original,
    /// 1.0
    Square,
    /// 1.25
    R5to4,
    /// 1.33
    R4to3,
    /// 1.5
    R3to2,
    /// 1.77
    R16to9,
}

#[derive(Debug, Clone, Copy, Default, glib::Enum, glib::Variant)]
#[enum_type(name = "LpOrientation")]
pub enum LpOrientation {
    #[default]
    Landscape,
    Portrait,
}

mod imp {
    use super::*;

    #[derive(Debug, Default, gtk::CompositeTemplate, glib::Properties)]
    #[properties(wrapper_type = super::LpEditCrop)]
    #[template(file = "crop.ui")]
    pub struct LpEditCrop {
        #[template_child]
        pub(super) image: TemplateChild<LpImage>,
        #[template_child]
        pub(super) selection: TemplateChild<LpEditCropSelection>,
        #[template_child]
        mode_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub(super) adjust: TemplateChild<LpEditAdjust>,

        #[property(get, construct_only)]
        original_image: OnceCell<LpImage>,

        #[property(get, set)]
        child: OnceCell<gtk::Widget>,

        /// Active editing view: `crop`, `adjust` or `markup`
        #[property(get, set = Self::set_mode, explicit_notify)]
        mode: RefCell<String>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for LpEditCrop {
        const NAME: &'static str = "LpEditCrop";
        type Type = super::LpEditCrop;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            LpEditAdjust::static_type();
            Self::bind_template(klass);

            klass.install_action("edit-crop.mirror-horizontally", None, |obj, _, _| {
                obj.imp().apply_mirror_horizontally();
            });
            klass.install_action("edit-crop.mirror-vertically", None, |obj, _, _| {
                obj.imp().apply_mirror_vertically()
            });
            klass.install_action("edit-crop.rotate-cw", None, |obj, _, _| {
                obj.imp().apply_rotate_cw()
            });
            klass.install_action("edit-crop.rotate-ccw", None, |obj, _, _| {
                obj.imp().apply_rotate_ccw()
            });
            klass.install_action("edit-crop.reset", None, |obj, _, _| {
                obj.handle_error(obj.imp().apply_reset())
            });
            klass.install_action("edit-crop.apply-crop", None, |obj, _, _| {
                obj.imp().apply_crop()
            });

            klass.add_shortcut(
                &gtk::Shortcut::builder()
                    .action(&gtk::NamedAction::new("edit-crop.mirror-horizontally"))
                    .trigger(&gtk::KeyvalTrigger::new(
                        gdk::Key::H,
                        gdk::ModifierType::NO_MODIFIER_MASK,
                    ))
                    .build(),
            );

            klass.add_shortcut(
                &gtk::Shortcut::builder()
                    .action(&gtk::NamedAction::new("edit-crop.mirror-vertically"))
                    .trigger(&gtk::KeyvalTrigger::new(
                        gdk::Key::V,
                        gdk::ModifierType::NO_MODIFIER_MASK,
                    ))
                    .build(),
            );

            klass.add_shortcut(
                &gtk::Shortcut::builder()
                    .action(&gtk::NamedAction::new("edit-crop.rotate-cw"))
                    .trigger(&gtk::KeyvalTrigger::new(
                        gdk::Key::R,
                        gdk::ModifierType::CONTROL_MASK,
                    ))
                    .build(),
            );

            klass.add_shortcut(
                &gtk::Shortcut::builder()
                    .action(&gtk::NamedAction::new("edit-crop.rotate-ccw"))
                    .trigger(&gtk::KeyvalTrigger::new(
                        gdk::Key::R,
                        gdk::ModifierType::CONTROL_MASK.union(gdk::ModifierType::SHIFT_MASK),
                    ))
                    .build(),
            );

            klass.add_shortcut(
                &gtk::Shortcut::builder()
                    .action(&gtk::NamedAction::new("edit-crop.apply-crop"))
                    .trigger(&gtk::KeyvalTrigger::new(
                        gdk::Key::Return,
                        gdk::ModifierType::CONTROL_MASK,
                    ))
                    .build(),
            );
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    #[glib::derived_properties]
    impl ObjectImpl for LpEditCrop {
        fn constructed(&self) {
            self.parent_constructed();

            let obj = &*self.obj();

            obj.child().set_parent(obj);

            self.image.duplicate_from(&obj.original_image());

            let actions = gio::SimpleActionGroup::new();
            actions.add_action(&gio::PropertyAction::new(
                "aspect-ratio",
                &*self.selection,
                "aspect_ratio",
            ));
            actions.add_action(&gio::PropertyAction::new(
                "orientation",
                &*self.selection,
                "orientation",
            ));

            obj.insert_action_group("edit-crop", Some(&actions));

            obj.action_set_enabled("edit-crop.reset", false);

            self.selection.connect_cropped_notify(glib::clone!(
                #[weak]
                obj,
                move |_| {
                    obj.action_set_enabled("edit-crop.reset", obj.imp().is_reset_enabled());
                }
            ));

            self.selection.connect_orientation_notify(glib::clone!(
                #[weak]
                obj,
                move |_| {
                    obj.imp().asepect_ratio_orientation_changed();
                }
            ));
        }

        fn dispose(&self) {
            self.obj().child().unparent();
        }
    }

    impl WidgetImpl for LpEditCrop {
        fn root(&self) {
            self.parent_root();
            let obj = self.obj();

            // Select correct orientation based on aspect ratio
            obj.handle_error(self.apply_reset());
        }
    }
    impl BinImpl for LpEditCrop {}

    impl LpEditCrop {
        fn reset_selection(&self) {
            self.selection.reset();
        }

        pub(super) fn apply_crop(&self) {
            // The selection is hidden and not kept up to date in other views
            if self.mode.borrow().as_str() != "crop" {
                return;
            }

            if let Some((x, y, width, height)) = self.selection.crop_area_image_coord() {
                // Rounding can put the selection a pixel outside the image
                let (image_width, image_height) = self.image.image_size();
                let x = x.min(image_width.max(1) as u32 - 1);
                let y = y.min(image_height.max(1) as u32 - 1);
                let width = width.clamp(1, image_width as u32 - x);
                let height = height.clamp(1, image_height as u32 - y);

                self.add_operation(Operation::Clip((x, y, width, height)));

                self.reset_selection();
            }
        }

        fn apply_mirror_horizontally(&self) {
            self.apply_crop();
            self.add_operation(Operation::MirrorHorizontally);

            self.reset_selection();
        }

        fn apply_mirror_vertically(&self) {
            self.apply_crop();
            self.add_operation(Operation::MirrorVertically);

            self.reset_selection();
        }

        fn apply_rotate_cw(&self) {
            self.apply_crop();
            self.add_operation(Operation::Rotate(gufo::common::orientation::Rotation::_270));

            self.reset_selection();
        }

        fn apply_rotate_ccw(&self) {
            self.apply_crop();
            self.add_operation(Operation::Rotate(gufo::common::orientation::Rotation::_90));

            self.reset_selection();
        }

        /// Undo all operations that can be done in this window
        fn apply_reset(&self) -> Result<(), EditingError> {
            let obj = self.obj();

            self.image
                .set_operations(Some(Arc::new(Operations::new(Vec::new()))))?;

            obj.edit_window().set_operations(None);
            self.reset_selection();

            let res = self.obj().activate_action(
                "edit-crop.aspect-ratio",
                Some(&LpAspectRatio::default().to_variant()),
            );
            if let Err(err) = res {
                tracing::error!("Failed to call action edit-crop.aspect-ratio: {err}");
            }

            let (w, h) = self.image.image_size();
            let orientation = if h > w {
                LpOrientation::Portrait
            } else {
                LpOrientation::Landscape
            };
            let res = self
                .obj()
                .activate_action("edit-crop.orientation", Some(&orientation.to_variant()));
            if let Err(err) = res {
                tracing::error!("Failed to call action edit-crop.orientation: {err}");
            }

            self.obj().action_set_enabled("edit-crop.reset", false);

            Ok(())
        }

        fn add_operation(&self, operation: Operation) {
            let obj = self.obj();

            let edit_window = obj.edit_window();
            let previous_operiatons = edit_window.operations();
            edit_window.add_operation(operation);
            let res = self.image.set_operations(edit_window.operations());

            if res.is_err() {
                obj.handle_error(res);
                // Reset to set of hopefully working operations
                edit_window.set_operations(previous_operiatons);
            } else {
                obj.action_set_enabled("edit-crop.reset", true);
            }
        }

        fn is_reset_enabled(&self) -> bool {
            self.obj().edit_window().operations().is_some() || self.selection.cropped()
        }

        fn asepect_ratio_orientation_changed(&self) {
            let obj = self.obj();
            match self.selection.orientation() {
                LpOrientation::Landscape => {
                    obj.remove_css_class("button-icons-portrait");
                }
                LpOrientation::Portrait => {
                    obj.add_css_class("button-icons-portrait");
                }
            }
        }

        fn set_mode(&self, mode: String) {
            if *self.mode.borrow() == mode {
                return;
            }

            let obj = self.obj();

            // Leaving the crop view commits the selection, like rotating does
            if self.mode.borrow().as_str() == "crop" {
                self.apply_crop();
            }

            let is_crop = mode == "crop";
            self.selection.set_visible(is_crop);
            self.selection.set_can_target(is_crop);
            for action in [
                "edit-crop.apply-crop",
                "edit-crop.mirror-horizontally",
                "edit-crop.mirror-vertically",
                "edit-crop.rotate-cw",
                "edit-crop.rotate-ccw",
            ] {
                obj.action_set_enabled(action, is_crop);
            }
            // The mode is first set while the widget is constructed
            let reset_enabled = obj.try_edit_window().is_some() && self.is_reset_enabled();
            obj.action_set_enabled("edit-crop.reset", is_crop && reset_enabled);

            if self.mode_stack.child_by_name(&mode).is_some() {
                self.mode_stack.set_visible_child_name(&mode);
            }

            if mode == "adjust" {
                self.adjust.sync();
            }

            self.mode.replace(mode);
            obj.notify_mode();
        }
    }
}

glib::wrapper! {
    pub struct LpEditCrop(ObjectSubclass<imp::LpEditCrop>)
    @extends gtk::Widget, adw::Bin,
    @implements gtk::Buildable, gtk::Accessible, gtk::ConstraintTarget;
}

impl LpEditCrop {
    pub fn new(original_image: LpImage) -> Self {
        let obj: Self = glib::Object::builder()
            .property("original_image", original_image)
            .build();

        obj
    }

    pub fn selection(&self) -> LpEditCropSelection {
        self.imp().selection.clone()
    }

    fn handle_error(&self, res: Result<(), EditingError>) {
        if let Err(err) = res {
            self.window_show_error(
                &gettext("Failed to Edit Image"),
                &format!("Failed to edit image: {err}"),
                ErrorType::General,
            );
        }
    }

    pub fn apply_crop(&self) {
        self.imp().apply_crop();
    }

    pub fn image(&self) -> LpImage {
        self.imp().image.clone()
    }
}
