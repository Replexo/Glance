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

//! Live text on top of an image
//!
//! Reads the text of the current image in the background. Found words
//! can then be selected by dragging over them with the mouse, or by
//! double-clicking a word, like text in a document. Only the Copy pill takes input itself.
//! Selecting is done by a gesture on the widget below, see `attach_input`,
//! so scrolling, zooming, clicks and touch still reach the image.

use std::cell::{Cell, OnceCell, RefCell};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use adw::prelude::*;
use adw::subclass::prelude::*;
use glib::{Properties, clone};

use crate::deps::*;
use crate::ocr::{self, Word};
use crate::util::gettext::*;
use crate::widgets::{LpImage, LpWindow};

/// Wait before reading, so quickly browsing images does not read each one
const START_DELAY: Duration = Duration::from_millis(150);
/// Extra space around words, for hitting and drawing them
const WORD_PADDING: f32 = 3.;
const PILL_SPACING: i32 = 8;
/// Movement before a press on a word becomes a selection
const DRAG_THRESHOLD: f64 = 4.;
/// Gap between words, in word heights, that splits a highlight band
const MAX_GAP: f32 = 1.5;

const ACCENT: gdk::RGBA = gdk::RGBA::new(0.208, 0.518, 0.894, 1.);

mod imp {
    use super::*;

    #[derive(Debug, Default, Properties)]
    #[properties(wrapper_type = super::LpTextLayer)]
    pub struct LpTextLayer {
        pub(super) image: OnceCell<LpImage>,
        /// The image is the current one and its text should be read
        pub(super) active: Cell<bool>,
        /// Text was found in the image
        #[property(get)]
        pub(super) has_text: Cell<bool>,

        pub(super) words: RefCell<Vec<Word>>,
        /// Rotation and mirroring the words were read with
        pub(super) read_orientation: Cell<Option<(i64, bool)>>,
        /// Rotation and mirroring of the reading that is scheduled or running
        pub(super) pending_orientation: Cell<Option<(i64, bool)>>,
        pub(super) cancelled: RefCell<Option<Arc<AtomicBool>>>,
        /// Stops loading the image for reading
        pub(super) cancellable: RefCell<Option<gio::Cancellable>>,
        pub(super) start_timeout: RefCell<Option<glib::SourceId>>,
        pub(super) adjustment_handlers: RefCell<Vec<(gtk::Adjustment, glib::SignalHandlerId)>>,

        /// First and last selected word
        pub(super) selection: Cell<Option<(usize, usize)>>,
        /// Word a press started on, before it becomes a selection
        pub(super) anchor: Cell<Option<usize>>,
        pub(super) drag_start: Cell<(f64, f64)>,
        pub(super) dragging: Cell<bool>,

        pub(super) pill: OnceCell<gtk::Box>,
        /// A press started on the pill
        pub(super) pill_pressed: Cell<bool>,
        /// Copy and Select All
        pub(super) pill_buttons: OnceCell<(gtk::Button, gtk::Button)>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for LpTextLayer {
        const NAME: &'static str = "LpTextLayer";
        type Type = super::LpTextLayer;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("text-layer");
        }
    }

    #[glib::derived_properties]
    impl ObjectImpl for LpTextLayer {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();

            obj.set_overflow(gtk::Overflow::Hidden);

            let pill = gtk::Box::builder()
                .css_classes(["text-pill"])
                .visible(false)
                .build();
            // Clicks on the pill are handled in `attach_input`, so a press on
            // its padding never reaches the image below
            let copy = gtk::Button::builder()
                .label(gettext("Copy"))
                .can_target(false)
                .focusable(false)
                .build();
            let select_all = gtk::Button::builder()
                .label(gettext("Select All"))
                .can_target(false)
                .focusable(false)
                .build();
            pill.append(&copy);
            pill.append(&gtk::Separator::new(gtk::Orientation::Vertical));
            pill.append(&select_all);
            pill.set_cursor_from_name(Some("default"));
            pill.set_parent(&*obj);
            self.pill.set(pill).unwrap();
            self.pill_buttons.set((copy, select_all)).unwrap();
        }

        fn dispose(&self) {
            self.cancel();
            for (adjustment, handler) in self.adjustment_handlers.take() {
                adjustment.disconnect(handler);
            }
            if let Some(pill) = self.pill.get() {
                pill.unparent();
            }
        }
    }

    impl WidgetImpl for LpTextLayer {
        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let (_, natural, _, _) = self.pill().measure(orientation, for_size);
            (0, natural, -1, -1)
        }

        fn size_allocate(&self, width: i32, height: i32, _baseline: i32) {
            let pill = self.pill();
            let (_, natural) = pill.preferred_size();
            let (pill_width, pill_height) = (natural.width(), natural.height());

            let (x, y) = match self.selection_bounds() {
                Some(bounds) => {
                    let x = bounds.x() + bounds.width() / 2. - pill_width as f32 / 2.;
                    let above = bounds.y() - PILL_SPACING as f32 - pill_height as f32;
                    let y = if above >= PILL_SPACING as f32 {
                        above
                    } else {
                        bounds.y() + bounds.height() + PILL_SPACING as f32
                    };
                    (x as i32, y as i32)
                }
                None => (0, 0),
            };

            let max_x = (width - pill_width - PILL_SPACING).max(PILL_SPACING);
            let max_y = (height - pill_height - PILL_SPACING).max(PILL_SPACING);
            let x = x.clamp(PILL_SPACING, max_x);
            let y = y.clamp(PILL_SPACING, max_y);

            pill.size_allocate(&gtk::Allocation::new(x, y, pill_width, pill_height), -1);
        }

        fn contains(&self, x: f64, y: f64) -> bool {
            self.pill_contains(x, y)
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let words = self.words.borrow();
            if !self.words_match_image() || words.is_empty() {
                return;
            }

            if let Some((start, end)) = self.selection.get()
                && let Some(selected) = words.get(start..=end)
            {
                let mut color = ACCENT;
                color.set_alpha(0.4);
                let fill = |rect: &graphene::Rect| {
                    let rounded = rounded(rect);
                    snapshot.push_rounded_clip(&rounded);
                    snapshot.append_color(&color, rect);
                    snapshot.pop();
                };

                // Level lines get one band each, tilted words their own tilted box
                let (level, tilted): (Vec<Word>, Vec<Word>) =
                    selected.iter().cloned().partition(|word| word.angle == 0.);
                for rect in self.line_rects(&level) {
                    fill(&rect);
                }
                for word in &tilted {
                    let shape = self.word_shape(word, WORD_PADDING);
                    snapshot.save();
                    snapshot.translate(&shape.center);
                    snapshot.rotate(shape.angle);
                    fill(&graphene::Rect::new(
                        -shape.width / 2.,
                        -shape.height / 2.,
                        shape.width,
                        shape.height,
                    ));
                    snapshot.restore();
                }
            }

            drop(words);
            self.obj().snapshot_child(self.pill(), snapshot);
        }
    }

    impl LpTextLayer {
        pub(super) fn image(&self) -> &LpImage {
            self.image.get().unwrap()
        }

        pub(super) fn pill(&self) -> &gtk::Box {
            self.pill.get().unwrap()
        }

        /// Current rotation in whole quarter turns, `None` while rotating
        pub(super) fn orientation(&self) -> Option<(i64, bool)> {
            let image = self.image();
            let rotation = image.rotation();
            let quarter = (rotation / 90.).round();
            ((rotation - quarter * 90.).abs() < 0.01)
                .then(|| (((quarter as i64) * 90).rem_euclid(360), image.mirrored()))
        }

        pub(super) fn words_match_image(&self) -> bool {
            let orientation = self.orientation();
            orientation.is_some() && orientation == self.read_orientation.get()
        }

        pub(super) fn cancel(&self) {
            if let Some(source) = self.start_timeout.take() {
                source.remove();
            }
            if let Some(cancelled) = self.cancelled.take() {
                cancelled.store(true, Ordering::Relaxed);
            }
            if let Some(cancellable) = self.cancellable.take() {
                cancellable.cancel();
            }
            self.pending_orientation.set(None);
        }

        /// Centre, size and tilt of a word in widget coordinates
        pub(super) fn word_shape(&self, word: &Word, padding: f32) -> WordShape {
            let image = self.image();
            // The image can be offset, for example by a scrollbar on the left
            let origin = image
                .compute_point(&*self.obj(), &graphene::Point::zero())
                .unwrap_or(graphene::Point::zero());
            let (x0, y0) = image.img_to_widget_coord((word.x, word.y));
            let (x1, y1) = image.img_to_widget_coord((word.x + word.width, word.y + word.height));
            WordShape {
                center: graphene::Point::new(
                    origin.x() + ((x0 + x1) / 2.) as f32,
                    origin.y() + ((y0 + y1) / 2.) as f32,
                ),
                width: (x1 - x0) as f32 + 2. * padding,
                height: (y1 - y0) as f32 + 2. * padding,
                angle: word.angle as f32,
            }
        }

        /// Level rectangle around a word in widget coordinates
        pub(super) fn word_rect(&self, word: &Word, padding: f32) -> graphene::Rect {
            self.word_shape(word, padding).bounds()
        }

        /// One band per run of words in a line, spanning the given words
        ///
        /// Large gaps end a band, so a band never stretches over a logo or
        /// an empty part of the image between two pieces of text.
        pub(super) fn line_rects(&self, words: &[Word]) -> Vec<graphene::Rect> {
            let mut rects = Vec::new();
            let mut current: Option<(usize, graphene::Rect)> = None;
            for word in words {
                let rect = self.word_rect(word, WORD_PADDING);
                match &mut current {
                    Some((line, band))
                        if *line == word.line
                            && rect.x() - (band.x() + band.width())
                                < rect.height().min(band.height()) * MAX_GAP =>
                    {
                        *band = band.union(&rect);
                    }
                    _ => {
                        rects.extend(current.take().map(|(_, band)| band));
                        current = Some((word.line, rect));
                    }
                }
            }
            rects.extend(current.map(|(_, band)| band));
            rects
        }

        pub(super) fn selection_bounds(&self) -> Option<graphene::Rect> {
            let (start, end) = self.selection.get()?;
            let words = self.words.borrow();
            self.line_rects(words.get(start..=end)?)
                .into_iter()
                .reduce(|a, b| a.union(&b))
        }

        /// Word at a widget position, or the closest one with `nearest`
        pub(super) fn word_at(&self, x: f64, y: f64, nearest: bool) -> Option<usize> {
            if !self.words_match_image() {
                return None;
            }

            let words = self.words.borrow();
            let point = graphene::Point::new(x as f32, y as f32);
            if let Some(index) = words
                .iter()
                .position(|word| self.word_shape(word, WORD_PADDING).contains(&point))
            {
                return Some(index);
            }

            if !nearest {
                return None;
            }

            // Closest line first, then the closest word in it
            let rects: Vec<_> = words.iter().map(|w| self.word_rect(w, 0.)).collect();
            let distance = |start: f32, length: f32, value: f32| {
                if value < start {
                    start - value
                } else {
                    (value - start - length).max(0.)
                }
            };
            let line = (0..words.len())
                .min_by(|a, b| {
                    let a = distance(rects[*a].y(), rects[*a].height(), y as f32);
                    let b = distance(rects[*b].y(), rects[*b].height(), y as f32);
                    a.total_cmp(&b)
                })
                .map(|index| words[index].line)?;

            (0..words.len())
                .filter(|index| words[*index].line == line)
                .min_by(|a, b| {
                    let a = distance(rects[*a].x(), rects[*a].width(), x as f32);
                    let b = distance(rects[*b].x(), rects[*b].width(), x as f32);
                    a.total_cmp(&b)
                })
        }

        /// The highlight of the selection covers `(x, y)`
        pub(super) fn selection_contains(&self, x: f64, y: f64) -> bool {
            let Some((start, end)) = self.selection.get() else {
                return false;
            };
            let words = self.words.borrow();
            let Some(selected) = words.get(start..=end) else {
                return false;
            };
            let point = graphene::Point::new(x as f32, y as f32);
            self.line_rects(selected)
                .iter()
                .any(|rect| rect.contains_point(&point))
        }

        /// First and last word of the line `index` is in
        pub(super) fn line_range(&self, index: usize) -> (usize, usize) {
            let words = self.words.borrow();
            let line = words[index].line;
            let start = words[..index]
                .iter()
                .rposition(|word| word.line != line)
                .map_or(0, |i| i + 1);
            let end = words[index..]
                .iter()
                .position(|word| word.line != line)
                .map_or(words.len() - 1, |i| index + i - 1);
            (start, end)
        }

        pub(super) fn pill_contains(&self, x: f64, y: f64) -> bool {
            let pill = self.pill();
            pill.is_visible()
                && pill
                    .compute_bounds(&*self.obj())
                    .is_some_and(|b| b.contains_point(&graphene::Point::new(x as f32, y as f32)))
        }

        /// Index of the pill button at a widget position
        pub(super) fn pill_button_at(&self, x: f64, y: f64) -> Option<usize> {
            if !self.pill().is_visible() {
                return None;
            }

            let point = graphene::Point::new(x as f32, y as f32);
            let (copy, select_all) = self.pill_buttons.get()?;
            [copy, select_all].iter().position(|button| {
                button
                    .compute_bounds(&*self.obj())
                    .is_some_and(|bounds| bounds.contains_point(&point))
            })
        }

        /// A press on a word may become a selection, everything else is
        /// left to the image
        pub(super) fn on_drag_begin(&self, gesture: &gtk::GestureDrag, x: f64, y: f64) {
            let is_touch =
                gesture.device().map(|x| x.source()) == Some(gdk::InputSource::Touchscreen);
            let has_modifiers = !(gesture.current_event_state()
                & gtk::accelerator_get_default_mod_mask())
            .is_empty();

            match self.word_at(x, y, false) {
                Some(index) if !is_touch && !has_modifiers && !self.pill_contains(x, y) => {
                    self.anchor.set(Some(index));
                    self.drag_start.set((x, y));
                }
                // Without an anchor the gesture stays idle and never claims
                _ => self.anchor.set(None),
            }
        }

        pub(super) fn on_drag_update(&self, gesture: &gtk::GestureDrag, dx: f64, dy: f64) {
            let Some(anchor) = self.anchor.get() else {
                return;
            };

            if !self.dragging.get() {
                // Small movements are clicks, which belong to the image
                if dx.hypot(dy) < DRAG_THRESHOLD {
                    return;
                }
                gesture.set_state(gtk::EventSequenceState::Claimed);
                self.dragging.set(true);
                self.pill().set_visible(false);
            }

            let (x, y) = self.drag_start.get();
            if let Some(index) = self.word_at(x + dx, y + dy, true) {
                self.selection
                    .set(Some((anchor.min(index), anchor.max(index))));
                self.obj().queue_draw();
            }
        }

        pub(super) fn on_drag_end(&self) {
            self.anchor.set(None);
            if self.dragging.replace(false) {
                self.update_pill();
            }
        }

        pub(super) fn update_pill(&self) {
            let visible = self.selection.get().is_some() && self.words_match_image();
            self.pill().set_visible(visible);
            self.obj().queue_allocate();
            self.obj().queue_draw();
        }
    }
}

glib::wrapper! {
    pub struct LpTextLayer(ObjectSubclass<imp::LpTextLayer>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for LpTextLayer {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl LpTextLayer {
    /// Shows the text of `image`, which must be drawn below this widget
    pub fn set_image(&self, image: &LpImage) {
        let imp = self.imp();
        if imp.image.set(image.clone()).is_err() {
            return;
        }

        for property in ["zoom", "rotation", "mirrored"] {
            image.connect_notify_local(
                Some(property),
                clone!(
                    #[weak(rename_to = obj)]
                    self,
                    move |_, _| obj.on_image_moved()
                ),
            );
        }
        for property in ["hadjustment", "vadjustment"] {
            image.connect_notify_local(
                Some(property),
                clone!(
                    #[weak(rename_to = obj)]
                    self,
                    move |_, _| obj.connect_adjustments()
                ),
            );
        }
        self.connect_adjustments();

        image.connect_is_loaded_notify(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_| obj.restart()
        ));
        // Emitted when the file is loaded again, for example after saving
        image.connect_local(
            "metadata-changed",
            false,
            clone!(
                #[weak(rename_to = obj)]
                self,
                #[upgrade_or_default]
                move |_| {
                    obj.restart();
                    None
                }
            ),
        );
    }

    /// Selects text by dragging over `widget`, which must have the same
    /// coordinates as this layer
    ///
    /// Only drags with the mouse that start on a word are taken. The gesture
    /// runs in the capture phase, so it decides before the image does.
    pub fn attach_input(&self, widget: &impl IsA<gtk::Widget>) {
        let drag = gtk::GestureDrag::builder()
            .button(1)
            .propagation_phase(gtk::PropagationPhase::Capture)
            .build();
        drag.connect_drag_begin(clone!(
            #[weak(rename_to = obj)]
            self,
            move |gesture, x, y| obj.imp().on_drag_begin(gesture, x, y)
        ));
        drag.connect_drag_update(clone!(
            #[weak(rename_to = obj)]
            self,
            move |gesture, dx, dy| obj.imp().on_drag_update(gesture, dx, dy)
        ));
        drag.connect_drag_end(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_, _, _| obj.imp().on_drag_end()
        ));
        drag.connect_cancel(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_, _| obj.imp().on_drag_end()
        ));
        widget.add_controller(drag);

        // Text cursor over words. The image only sets its own cursor while panning.
        let motion = gtk::EventControllerMotion::new();
        motion.connect_motion(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_, x, y| obj.update_cursor(Some((x, y)))
        ));
        motion.connect_leave(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_| obj.update_cursor(None)
        ));
        widget.add_controller(motion);

        // Clicks on the pill press its buttons, clicks next to the text end
        // its selection
        let click = gtk::GestureClick::builder()
            .button(1)
            .propagation_phase(gtk::PropagationPhase::Capture)
            .build();
        click.connect_pressed(clone!(
            #[weak(rename_to = obj)]
            self,
            move |gesture, n_press, x, y| {
                let imp = obj.imp();
                let on_pill = imp.pill_contains(x, y);
                imp.pill_pressed.set(on_pill);
                if on_pill {
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                } else if n_press >= 2
                    && (imp.word_at(x, y, false).is_some() || imp.selection_contains(x, y))
                {
                    // Double and triple clicks on text select it, they never
                    // reach the image, which would go fullscreen
                    gesture.set_state(gtk::EventSequenceState::Claimed);
                    if let Some(index) = imp.word_at(x, y, true) {
                        let range = if n_press == 2 {
                            (index, index)
                        } else {
                            imp.line_range(index)
                        };
                        imp.selection.set(Some(range));
                        imp.update_pill();
                    }
                } else {
                    obj.clear_selection();
                }
            }
        ));
        click.connect_released(clone!(
            #[weak(rename_to = obj)]
            self,
            move |_, _, x, y| {
                if !obj.imp().pill_pressed.replace(false) {
                    return;
                }
                match obj.imp().pill_button_at(x, y) {
                    Some(0) => {
                        obj.copy_selection();
                    }
                    Some(_) => obj.select_all(),
                    None => {}
                }
            }
        ));
        widget.add_controller(click);
    }

    fn update_cursor(&self, position: Option<(f64, f64)>) {
        let imp = self.imp();
        let image = imp.image();
        let current = image.cursor().map(|x| x.name());
        let is_text = current
            .as_ref()
            .is_some_and(|x| x.as_deref() == Some("text"));
        if current.is_some() && !is_text {
            // Panning
            return;
        }

        let over_word =
            imp.dragging.get() || position.is_some_and(|(x, y)| imp.word_at(x, y, false).is_some());
        if over_word != is_text {
            if over_word {
                image.set_cursor_from_name(Some("text"));
            } else {
                image.set_cursor(None);
            }
        }
    }

    fn connect_adjustments(&self) {
        let imp = self.imp();
        for (adjustment, handler) in imp.adjustment_handlers.take() {
            adjustment.disconnect(handler);
        }

        let image = imp.image();
        let handlers = [image.hadjustment(), image.vadjustment()]
            .into_iter()
            .flatten()
            .map(|adjustment| {
                let handler = adjustment.connect_value_changed(clone!(
                    #[weak(rename_to = obj)]
                    self,
                    move |_| obj.on_image_moved()
                ));
                (adjustment, handler)
            })
            .collect();
        imp.adjustment_handlers.replace(handlers);
    }

    fn on_image_moved(&self) {
        let imp = self.imp();
        let orientation = imp.orientation();
        if imp.active.get()
            && orientation.is_some()
            && orientation != imp.read_orientation.get()
            && orientation != imp.pending_orientation.get()
        {
            self.restart();
        }

        if !imp.words_match_image() {
            self.clear_selection();
        } else if imp.selection.get().is_some() {
            self.queue_allocate();
        }
        self.queue_draw();
    }

    /// Reads the text while `active`, and stops reading otherwise
    pub fn set_active(&self, active: bool) {
        let imp = self.imp();
        if imp.active.replace(active) == active {
            return;
        }

        if !active {
            imp.cancel();
            self.clear_selection();
        } else if !imp.words_match_image() {
            self.restart();
        }
    }

    /// Forgets found text and reads the image again
    fn restart(&self) {
        let imp = self.imp();
        imp.cancel();
        imp.words.borrow_mut().clear();
        imp.read_orientation.set(None);
        imp.selection.set(None);
        imp.anchor.set(None);
        imp.pill().set_visible(false);
        imp.has_text.set(false);
        self.notify_has_text();
        self.queue_draw();

        let image = imp.image();
        let (true, true, Some(orientation), Some(file)) = (
            imp.active.get(),
            image.is_loaded(),
            imp.orientation(),
            image.file(),
        ) else {
            return;
        };

        let cancelled = Arc::new(AtomicBool::new(false));
        imp.cancelled.replace(Some(cancelled.clone()));
        let cancellable = gio::Cancellable::new();
        imp.cancellable.replace(Some(cancellable.clone()));
        imp.pending_orientation.set(Some(orientation));

        let weak = self.downgrade();
        let source = glib::timeout_add_local_once(START_DELAY, move || {
            let Some(obj) = weak.upgrade() else {
                return;
            };
            obj.imp().start_timeout.take();
            drop(obj);

            // Only a weak reference is kept while reading, so closing the
            // page drops the layer and cancels the reading
            glib::spawn_future_local(async move {
                let result = ocr::recognize(
                    file,
                    orientation.0 as f64,
                    orientation.1,
                    cancelled.clone(),
                    cancellable,
                )
                .await;

                if let Some(obj) = weak.upgrade() {
                    obj.finish_read(result, orientation, &cancelled);
                }
            });
        });
        imp.start_timeout.replace(Some(source));
    }

    fn finish_read(
        &self,
        result: anyhow::Result<Vec<Word>>,
        orientation: (i64, bool),
        cancelled: &Arc<AtomicBool>,
    ) {
        let imp = self.imp();
        let is_current = imp
            .cancelled
            .borrow()
            .as_ref()
            .is_some_and(|x| Arc::ptr_eq(x, cancelled));
        if !is_current || cancelled.load(Ordering::Relaxed) {
            return;
        }
        imp.cancelled.take();
        imp.cancellable.take();
        imp.pending_orientation.set(None);

        // A failed read counts as no text, so moving the image does not retry it
        let words = result.unwrap_or_else(|err| {
            tracing::debug!("Could not read text in image: {err}");
            Vec::new()
        });

        tracing::debug!("Found {} words in image", words.len());
        let has_text = !words.is_empty();
        imp.words.replace(words);
        imp.read_orientation.set(Some(orientation));
        imp.selection.set(None);
        imp.has_text.set(has_text);
        self.notify_has_text();
    }

    pub fn clear_selection(&self) {
        let imp = self.imp();
        if imp.selection.take().is_some() {
            imp.update_pill();
        }
    }

    /// Selects all text
    pub fn select_all(&self) {
        let imp = self.imp();
        let count = imp.words.borrow().len();
        if count == 0 || !imp.words_match_image() {
            return;
        }

        imp.selection.set(Some((0, count - 1)));
        imp.update_pill();
    }

    /// Copies the selected text, returns whether there was any
    pub fn copy_selection(&self) -> bool {
        let imp = self.imp();
        let Some((start, end)) = imp.selection.get() else {
            return false;
        };
        let Some(text) = imp.words.borrow().get(start..=end).map(ocr::join_words) else {
            return false;
        };

        self.clipboard().set_text(&text);

        if let Some(window) = self.root().and_downcast::<LpWindow>() {
            window.show_toast(
                &gettext("Text copied to clipboard"),
                adw::ToastPriority::High,
            );
        }

        true
    }
}

fn rounded(rect: &graphene::Rect) -> gsk::RoundedRect {
    let radius = (rect.height() / 4.).min(6.);
    gsk::RoundedRect::from_rect(*rect, radius)
}

/// A word's box in widget coordinates, turned clockwise by `angle` degrees
/// around its centre
pub(super) struct WordShape {
    center: graphene::Point,
    width: f32,
    height: f32,
    angle: f32,
}

impl WordShape {
    /// Turns a point into the word's own unturned frame
    fn to_local(&self, point: &graphene::Point) -> (f32, f32) {
        let (sin, cos) = (-self.angle).to_radians().sin_cos();
        let dx = point.x() - self.center.x();
        let dy = point.y() - self.center.y();
        (dx * cos - dy * sin, dx * sin + dy * cos)
    }

    fn contains(&self, point: &graphene::Point) -> bool {
        let (x, y) = self.to_local(point);
        x.abs() <= self.width / 2. && y.abs() <= self.height / 2.
    }

    fn bounds(&self) -> graphene::Rect {
        let (sin, cos) = self.angle.to_radians().sin_cos();
        let half_width = (self.width * cos.abs() + self.height * sin.abs()) / 2.;
        let half_height = (self.width * sin.abs() + self.height * cos.abs()) / 2.;
        graphene::Rect::new(
            self.center.x() - half_width,
            self.center.y() - half_height,
            2. * half_width,
            2. * half_height,
        )
    }
}
