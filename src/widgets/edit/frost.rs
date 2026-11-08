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

//! Blurs the picture behind the floating pills
//!
//! GTK CSS has no backdrop filter, so this sits in a `GtkOverlay` under the
//! pills and draws a blurred copy of the picture where each pill is. The
//! picture is the first `LpImage` in the overlay's main child, and the pills
//! are the other overlay children with the `glance-pill` or `glance-name`
//! style class.

use std::cell::{OnceCell, RefCell};

use adw::prelude::*;
use adw::subclass::prelude::*;

use crate::deps::*;
use crate::widgets::LpImage;

const BLUR_RADIUS: f32 = 18.;
const CLASSES: [&str; 2] = ["glance-pill", "glance-name"];

/// Place of a pill in the frost's coordinates, and its opacity
type Pane = (graphene::Rect, f64);

mod imp {
    use super::*;

    #[derive(Debug, Default)]
    pub struct LpFrost {
        pub(super) paintable: OnceCell<gtk::WidgetPaintable>,
        /// Panes drawn last time, to redraw only when a pill changes
        pub(super) panes: RefCell<Vec<Pane>>,
        pub(super) layout_handler: RefCell<Option<(gdk::FrameClock, glib::SignalHandlerId)>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for LpFrost {
        const NAME: &'static str = "LpFrost";
        type Type = super::LpFrost;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for LpFrost {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_can_target(false);
            obj.set_can_focus(false);
        }
    }

    impl WidgetImpl for LpFrost {
        fn map(&self) {
            self.parent_map();
            // Pills move, appear and fade without resizing the frost, so
            // their places are checked whenever GTK lays out a frame
            let obj = self.obj();
            if let Some(clock) = obj.frame_clock() {
                let handler = clock.connect_layout(glib::clone!(
                    #[weak]
                    obj,
                    move |_| {
                        let imp = obj.imp();
                        if *imp.panes.borrow() != imp.find_panes() {
                            obj.queue_draw();
                        }
                    }
                ));
                self.layout_handler.replace(Some((clock, handler)));
            }
        }

        fn unmap(&self) {
            if let Some((clock, handler)) = self.layout_handler.take() {
                clock.disconnect(handler);
            }
            self.parent_unmap();
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let obj = self.obj();
            let panes = self.find_panes();
            self.panes.replace(panes.clone());
            if panes.is_empty() {
                return;
            }
            let Some(image) = self.image() else {
                return;
            };
            let Some(bounds) = image.compute_bounds(&*obj) else {
                return;
            };
            let paintable = self.paintable.get_or_init(|| {
                let paintable = gtk::WidgetPaintable::new(None::<&gtk::Widget>);
                paintable.connect_invalidate_contents(glib::clone!(
                    #[weak]
                    obj,
                    move |_| obj.queue_draw()
                ));
                paintable
            });
            if paintable.widget().as_ref() != Some(image.upcast_ref()) {
                paintable.set_widget(Some(&image));
            }

            for (rect, opacity) in panes {
                let radius = rect.width().min(rect.height()) / 2.;
                let rounded = gsk::RoundedRect::from_rect(rect, radius);
                snapshot.push_opacity(opacity);
                snapshot.push_rounded_clip(&rounded);
                snapshot.push_blur(BLUR_RADIUS as f64);
                // Only the area the blur reaches is drawn
                snapshot.push_clip(&rect.inset_r(-2. * BLUR_RADIUS, -2. * BLUR_RADIUS));
                snapshot.save();
                snapshot.translate(&graphene::Point::new(bounds.x(), bounds.y()));
                paintable.snapshot(snapshot, bounds.width() as f64, bounds.height() as f64);
                snapshot.restore();
                snapshot.pop();
                snapshot.pop();
                snapshot.pop();
                snapshot.pop();
            }
        }
    }

    impl LpFrost {
        fn overlay(&self) -> Option<gtk::Overlay> {
            self.obj().parent().and_downcast()
        }

        /// The picture in the overlay's main child
        fn image(&self) -> Option<LpImage> {
            let main = self.overlay()?.child()?;
            find_image(&main)
        }

        /// Pills that are shown in the other overlay children
        pub(super) fn find_panes(&self) -> Vec<Pane> {
            let obj = self.obj();
            let Some(overlay) = self.overlay() else {
                return Vec::new();
            };
            let main = overlay.child();
            let mut panes = Vec::new();
            let mut child = overlay.first_child();
            while let Some(widget) = child {
                if widget != *obj && Some(&widget) != main.as_ref() {
                    collect_panes(&widget, &obj, &overlay, &mut panes);
                }
                child = widget.next_sibling();
            }
            panes
        }
    }
}

fn find_image(widget: &gtk::Widget) -> Option<LpImage> {
    if let Some(image) = widget.downcast_ref::<LpImage>() {
        return Some(image.clone());
    }
    let mut child = widget.first_child();
    while let Some(widget) = child {
        if widget.is_drawable()
            && let Some(image) = find_image(&widget)
        {
            return Some(image);
        }
        child = widget.next_sibling();
    }
    None
}

fn collect_panes(
    widget: &gtk::Widget,
    frost: &LpFrost,
    overlay: &gtk::Overlay,
    panes: &mut Vec<Pane>,
) {
    if !widget.is_drawable() || widget.opacity() <= 0. {
        return;
    }
    if CLASSES.iter().any(|class| widget.has_css_class(class)) {
        if let Some(rect) = widget.compute_bounds(frost)
            && rect.width() > 0.
            && rect.height() > 0.
        {
            panes.push((rect, opacity(widget, overlay)));
        }
        return;
    }
    let mut child = widget.first_child();
    while let Some(widget) = child {
        collect_panes(&widget, frost, overlay, panes);
        child = widget.next_sibling();
    }
}

/// Opacity of `widget` including its parents up to `overlay`, for pills
/// that fade in and out
fn opacity(widget: &gtk::Widget, overlay: &gtk::Overlay) -> f64 {
    let mut opacity = 1.;
    let mut current = Some(widget.clone());
    while let Some(widget) = current {
        if widget == *overlay.upcast_ref::<gtk::Widget>() {
            break;
        }
        opacity *= widget.opacity();
        current = widget.parent();
    }
    opacity
}

glib::wrapper! {
    pub struct LpFrost(ObjectSubclass<imp::LpFrost>)
    @extends gtk::Widget,
    @implements gtk::Buildable, gtk::Accessible, gtk::ConstraintTarget;
}
