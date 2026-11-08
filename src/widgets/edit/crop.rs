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

use std::cell::{Cell, OnceCell, RefCell};
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
use crate::widgets::edit::markup::{font_from_name, tool_from_name, width_from_name};
use crate::widgets::edit::{
    LpEditAdjust, LpEditCropSelection, LpEditMarkup, LpFrost, LpMarkupCanvas, LpResizeDialog,
};

fn aspect_ratio_label(aspect_ratio: LpAspectRatio) -> String {
    match aspect_ratio {
        LpAspectRatio::Free => gettext("Free"),
        LpAspectRatio::Original => gettext("Original"),
        LpAspectRatio::Square => gettext("Square"),
        LpAspectRatio::R5to4 => "5:4".to_owned(),
        LpAspectRatio::R4to3 => "4:3".to_owned(),
        LpAspectRatio::R3to2 => "3:2".to_owned(),
        LpAspectRatio::R16to9 => "16:9".to_owned(),
    }
}

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
        canvas: TemplateChild<gtk::Overlay>,
        #[template_child]
        mode_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub(super) adjust: TemplateChild<LpEditAdjust>,
        #[template_child]
        pub(super) markup: TemplateChild<LpEditMarkup>,
        #[template_child]
        pub(super) markup_canvas: TemplateChild<LpMarkupCanvas>,
        #[template_child]
        pub(super) size_label: TemplateChild<gtk::Label>,
        #[template_child]
        aspect_button: TemplateChild<gtk::MenuButton>,
        #[template_child]
        aspect_content: TemplateChild<adw::ButtonContent>,
        #[template_child]
        aspect_scroll: TemplateChild<gtk::ScrolledWindow>,

        #[property(get, construct_only)]
        original_image: OnceCell<LpImage>,

        #[property(get, set)]
        child: OnceCell<gtk::Widget>,

        /// Active editing view: `crop`, `adjust` or `markup`
        #[property(get, set = Self::set_mode, explicit_notify)]
        mode: RefCell<String>,

        /// Markup tool in use, and the width each tool used last
        tool_name: RefCell<String>,
        tool_widths: RefCell<std::collections::HashMap<String, String>>,

        /// Pointer position, for zooming with the scroll wheel
        pointer: Cell<Option<(f64, f64)>>,
        /// Zoom level when a pinch started
        pinch_zoom: Cell<f64>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for LpEditCrop {
        const NAME: &'static str = "LpEditCrop";
        type Type = super::LpEditCrop;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            LpEditAdjust::static_type();
            LpEditMarkup::static_type();
            LpMarkupCanvas::static_type();
            LpFrost::static_type();
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
            klass.install_action("edit-crop.resize", None, |obj, _, _| {
                obj.imp().show_resize_dialog()
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
            // Same colour as the editor's canvas
            self.image
                .set_fixed_background_color(Some(gdk::RGBA::new(0.059, 0.059, 0.067, 1.)));
            self.setup_zoom();

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

            // Matches the tool action's first state
            self.tool_name.replace("brush".to_owned());
            let markup_actions = gio::SimpleActionGroup::new();
            let tool_action = gio::SimpleAction::new_stateful(
                "tool",
                Some(glib::VariantTy::STRING),
                &"brush".to_variant(),
            );
            tool_action.connect_activate(glib::clone!(
                #[weak]
                obj,
                move |action, target| {
                    if let Some(name) = target.and_then(|x| x.get::<String>())
                        && let Some(tool) = tool_from_name(&name)
                    {
                        action.set_state(&name.to_variant());
                        let imp = obj.imp();
                        imp.markup_canvas.set_tool(tool);
                        imp.markup.set_tool(tool);
                        // Each tool keeps its own width or size
                        imp.tool_name.replace(name.clone());
                        let width = imp
                            .tool_widths
                            .borrow()
                            .get(&name)
                            .cloned()
                            .unwrap_or_else(|| "m".to_owned());
                        let _ = obj.activate_action("edit-markup.width", Some(&width.to_variant()));
                    }
                }
            ));
            markup_actions.add_action(&tool_action);

            let color_action = gio::SimpleAction::new_stateful(
                "color",
                Some(glib::VariantTy::STRING),
                &"blue".to_variant(),
            );
            color_action.connect_activate(glib::clone!(
                #[weak]
                obj,
                move |action, target| {
                    if let Some(name) = target.and_then(|x| x.get::<String>())
                        && let Some(color) = obj.imp().markup.color(&name)
                    {
                        action.set_state(&name.to_variant());
                        let imp = obj.imp();
                        imp.markup_canvas.set_color(color);
                        imp.markup.show_color(&name);
                    }
                }
            ));
            markup_actions.add_action(&color_action);

            let fill_action = gio::SimpleAction::new_stateful(
                "fill",
                Some(glib::VariantTy::STRING),
                &"outline".to_variant(),
            );
            fill_action.connect_activate(glib::clone!(
                #[weak]
                obj,
                move |action, target| {
                    if let Some(name) = target.and_then(|x| x.get::<String>()) {
                        obj.imp().markup_canvas.set_filled(name == "filled");
                        action.set_state(&name.to_variant());
                    }
                }
            ));
            markup_actions.add_action(&fill_action);

            let width_action = gio::SimpleAction::new_stateful(
                "width",
                Some(glib::VariantTy::STRING),
                &"m".to_variant(),
            );
            width_action.connect_activate(glib::clone!(
                #[weak]
                obj,
                move |action, target| {
                    if let Some(name) = target.and_then(|x| x.get::<String>())
                        && let Some(factor) = width_from_name(&name)
                    {
                        let imp = obj.imp();
                        imp.markup_canvas.set_size_factor(factor);
                        action.set_state(&name.to_variant());
                        let tool = imp.tool_name.borrow().clone();
                        imp.tool_widths.borrow_mut().insert(tool, name);
                    }
                }
            ));
            markup_actions.add_action(&width_action);

            let font_action = gio::SimpleAction::new_stateful(
                "font",
                Some(glib::VariantTy::STRING),
                &"sans".to_variant(),
            );
            font_action.connect_activate(glib::clone!(
                #[weak]
                obj,
                move |action, target| {
                    if let Some(name) = target.and_then(|x| x.get::<String>())
                        && let Some(font) = font_from_name(&name)
                    {
                        obj.imp().markup_canvas.set_font(font);
                        action.set_state(&name.to_variant());
                    }
                }
            ));
            markup_actions.add_action(&font_action);
            obj.insert_action_group("edit-markup", Some(&markup_actions));
            self.markup.show_color("blue");
            // Keep a gap to the pill below
            if let Some(popover) = self.aspect_button.popover() {
                popover.set_offset(0, -10);
                // Centred over the whole pill and never wider than the window
                popover.connect_show(glib::clone!(
                    #[weak]
                    obj,
                    move |popover| {
                        let imp = obj.imp();
                        let button = imp.aspect_button.upcast_ref::<gtk::Widget>();
                        if let Some(pill) = button.parent()
                            && let Some(bounds) = pill.compute_bounds(button)
                        {
                            popover.set_pointing_to(Some(&gdk::Rectangle::new(
                                bounds.x() as i32,
                                bounds.y() as i32,
                                bounds.width() as i32,
                                bounds.height() as i32,
                            )));
                        }
                        let window_width = obj.width();
                        imp.aspect_scroll.set_max_content_width((window_width - 32).max(120));
                    }
                ));
            }

            self.selection.connect_aspect_ratio_notify(glib::clone!(
                #[weak]
                obj,
                move |selection| {
                    let imp = obj.imp();
                    imp.aspect_content
                        .set_label(&aspect_ratio_label(selection.aspect_ratio()));
                    imp.aspect_button.popdown();
                }
            ));

            for property in ["crop-x", "crop-y", "crop-width", "crop-height"] {
                self.selection.connect_notify_local(
                    Some(property),
                    glib::clone!(
                        #[weak]
                        obj,
                        move |_, _| obj.update_size_label()
                    ),
                );
            }

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
        /// Pinch, Ctrl+scroll and scrolling move the picture in Adjust and
        /// Markup; Crop always shows the whole picture
        fn setup_zoom(&self) {
            let obj = self.obj();

            // Without an adjustment that redraws, panning would not show
            self.image
                .set_property("hadjustment", gtk::Adjustment::default());
            self.image
                .set_property("vadjustment", gtk::Adjustment::default());
            for adjustment in [self.image.hadj(), self.image.vadj()] {
                adjustment.connect_value_changed(glib::clone!(
                    #[weak]
                    obj,
                    move |_| obj.imp().markup_canvas.queue_draw()
                ));
            }
            self.image.connect_zoom_notify(glib::clone!(
                #[weak]
                obj,
                move |_| {
                    let imp = obj.imp();
                    imp.markup_canvas.queue_draw();
                    // Best fit changes when the pills change height
                    if !imp.selection.cropped() {
                        imp.selection.reset();
                    }
                    imp.selection.queue_allocate();
                }
            ));

            // Best fit keeps the picture clear of the pills at the bottom
            self.canvas.connect_get_child_position(glib::clone!(
                #[weak]
                obj,
                #[upgrade_or]
                None,
                move |canvas, widget| {
                    let imp = obj.imp();
                    if widget == imp.mode_stack.upcast_ref::<gtk::Widget>() {
                        // The stack is centred, so it gets its natural size
                        let (_, width, _, _) = widget.measure(gtk::Orientation::Horizontal, -1);
                        let (_, pills, _, _) = widget
                            .measure(gtk::Orientation::Vertical, width.min(canvas.width()));
                        imp.image.set_fit_insets((18., 18., 18., pills as f64 + 12.));
                    }
                    None
                }
            ));

            let motion = gtk::EventControllerMotion::new();
            motion.connect_motion(glib::clone!(
                #[weak]
                obj,
                move |_, x, y| obj.imp().pointer.set(Some((x, y)))
            ));
            motion.connect_leave(glib::clone!(
                #[weak]
                obj,
                move |_| obj.imp().pointer.set(None)
            ));
            self.canvas.add_controller(motion);

            let scroll = gtk::EventControllerScroll::new(
                gtk::EventControllerScrollFlags::BOTH_AXES,
            );
            scroll.set_propagation_phase(gtk::PropagationPhase::Capture);
            scroll.connect_scroll(glib::clone!(
                #[weak]
                obj,
                #[upgrade_or]
                glib::Propagation::Proceed,
                move |controller, dx, dy| obj.imp().scroll(controller, dx, dy)
            ));
            self.canvas.add_controller(scroll);

            let pinch = gtk::GestureZoom::new();
            pinch.set_propagation_phase(gtk::PropagationPhase::Capture);
            pinch.connect_begin(glib::clone!(
                #[weak]
                obj,
                move |gesture, _| {
                    let imp = obj.imp();
                    if imp.zoomable() {
                        // Stops the markup tools from drawing with two fingers
                        gesture.set_state(gtk::EventSequenceState::Claimed);
                        imp.pinch_zoom.set(imp.image.zoom());
                    } else {
                        gesture.set_state(gtk::EventSequenceState::Denied);
                    }
                }
            ));
            pinch.connect_scale_changed(glib::clone!(
                #[weak]
                obj,
                move |gesture, scale| {
                    let imp = obj.imp();
                    if let Some(center) = gesture.bounding_box_center() {
                        imp.zoom_at(imp.pinch_zoom.get() * scale, center);
                    }
                }
            ));
            self.canvas.add_controller(pinch);
        }

        /// Scrolling over the pills is for their sliders and buttons
        fn pointer_over_pills(&self) -> bool {
            self.pointer.get().is_some_and(|(x, y)| {
                self.canvas
                    .pick(x, y, gtk::PickFlags::DEFAULT)
                    .is_some_and(|widget| widget.is_ancestor(&*self.mode_stack))
            })
        }

        pub(super) fn zoomable(&self) -> bool {
            self.mode.borrow().as_str() != "crop"
        }

        /// Zooms with the image point under `position` staying in place,
        /// never below best fit
        fn zoom_at(&self, zoom: f64, position: (f64, f64)) {
            let zoom = zoom.max(self.image.best_fit_zoom());
            self.image.zoom_at(zoom, position);
        }

        fn scroll(
            &self,
            controller: &gtk::EventControllerScroll,
            dx: f64,
            dy: f64,
        ) -> glib::Propagation {
            if !self.zoomable() || self.pointer_over_pills() {
                return glib::Propagation::Proceed;
            }

            let wheel = controller.unit() == gdk::ScrollUnit::Wheel;
            if controller
                .current_event_state()
                .contains(gdk::ModifierType::CONTROL_MASK)
            {
                let factor = if wheel {
                    1.25_f64.powf(-dy)
                } else {
                    (1. - dy / 200.).max(0.1)
                };
                let center = (
                    self.image.width() as f64 / 2.,
                    self.image.height() as f64 / 2.,
                );
                let position = self.pointer.get().unwrap_or(center);
                self.zoom_at(self.image.zoom() * factor, position);
                return glib::Propagation::Stop;
            }

            if self.image.is_best_fit() {
                return glib::Propagation::Proceed;
            }
            // Wheel steps are lines, touchpads give pixels
            let step = if wheel { 48. } else { 1. };
            let hadj = self.image.hadj();
            let vadj = self.image.vadj();
            hadj.set_value((hadj.value() + dx * step).clamp(0., hadj.upper() - hadj.page_size()));
            vadj.set_value((vadj.value() + dy * step).clamp(0., vadj.upper() - vadj.page_size()));
            glib::Propagation::Stop
        }

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

            obj.edit_window().reset_crop_view();
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
            edit_window.add_operation(operation);
            let res = self.image.set_operations(edit_window.operations());

            if res.is_err() {
                obj.handle_error(res);
                // Back to the set of hopefully working operations
                edit_window.discard_last_change();
            } else {
                obj.action_set_enabled("edit-crop.reset", true);
            }
        }

        pub(super) fn is_reset_enabled(&self) -> bool {
            let edit_window = self.obj().edit_window();
            edit_window.operations().is_some()
                || edit_window.resize().is_some()
                || self.selection.cropped()
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

        fn show_resize_dialog(&self) {
            let obj = self.obj();
            // The size shown includes a pending crop
            self.apply_crop();

            let edit_window = obj.edit_window();
            let (width, height) = self.image.image_size();
            let dialog = LpResizeDialog::new(
                (width.max(1) as u32, height.max(1) as u32),
                edit_window.resize().unwrap_or(1.),
            );
            dialog.connect_scale_chosen(glib::clone!(
                #[weak]
                edit_window,
                move |_, scale| edit_window.set_resize(Some(scale))
            ));
            dialog.present(Some(&*obj));
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
            // Leaving markup adds the text being typed
            if self.mode.borrow().as_str() == "markup" {
                self.markup_canvas.finish_text();
            }

            let is_crop = mode == "crop";
            // The crop selection needs the whole picture
            if is_crop && !self.image.is_best_fit() {
                let center = (
                    self.image.width() as f64 / 2.,
                    self.image.height() as f64 / 2.,
                );
                self.image.zoom_at(self.image.best_fit_zoom(), center);
            }
            self.size_label.set_visible(is_crop);
            if !is_crop {
                self.aspect_button.popdown();
            }
            self.selection.set_visible(is_crop);
            self.selection.set_can_target(is_crop);
            let is_markup = mode == "markup";
            self.markup_canvas.set_can_target(is_markup);
            self.markup_canvas.set_focusable(is_markup);
            self.markup_canvas.clear_selection();
            for action in [
                "edit-crop.apply-crop",
                "edit-crop.mirror-horizontally",
                "edit-crop.mirror-vertically",
                "edit-crop.rotate-cw",
                "edit-crop.rotate-ccw",
                "edit-crop.resize",
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

            if let Some(edit_window) = obj.try_edit_window() {
                edit_window.end_coalescing();
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

    /// Zooms in or out a step, or back to best fit, outside Crop
    pub fn zoom_step(&self, direction: i32) {
        let imp = self.imp();
        if !imp.zoomable() {
            return;
        }
        match direction {
            0 => imp.image.zoom_best_fit(),
            d if d > 0 => imp.image.zoom_in_center(),
            _ => imp.image.zoom_out_center(),
        }
    }

    pub fn markup_canvas(&self) -> LpMarkupCanvas {
        self.imp().markup_canvas.clone()
    }

    pub fn markup_changed(&self) {
        self.imp().markup_canvas.queue_draw();
    }

    /// Shows the size of the crop selection, and the size after resizing
    pub fn update_size_label(&self) {
        let imp = self.imp();
        let Some(edit_window) = self.try_edit_window() else {
            return;
        };

        let (width, height) = imp
            .selection
            .crop_area_image_coord()
            .map(|(_, _, w, h)| (w, h))
            .unwrap_or_else(|| {
                let (w, h) = imp.image.image_size();
                (w.max(0) as u32, h.max(0) as u32)
            });

        let mut label = format!("{width} × {height}");
        if let Some(scale) = edit_window.resize() {
            let (width, height) = crate::editing::render::scaled_size((width, height), scale);
            label.push_str(&format!("  →  {width} × {height}"));
        }
        imp.size_label.set_label(&label);

        self.action_set_enabled(
            "edit-crop.reset",
            self.mode() == "crop" && imp.is_reset_enabled(),
        );
    }

    /// Updates controls after undo or redo
    pub fn state_restored(&self) {
        let imp = self.imp();
        imp.adjust.sync();
        imp.markup_canvas.queue_draw();
        self.action_set_enabled(
            "edit-crop.reset",
            self.mode() == "crop" && imp.is_reset_enabled(),
        );
    }
}
