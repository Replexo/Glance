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

//! Shows the changelog of a new release and installs it

use adw::prelude::*;
use glib::clone;

use crate::application::LpApplication;
use crate::config;
use crate::deps::*;
use crate::updates::{InstallKind, Release};
use crate::util::gettext::*;

/// Parts of the dialog that change with its state
#[derive(Clone)]
struct Parts {
    dialog: adw::Dialog,
    title: gtk::Label,
    subtitle: gtk::Label,
    notes_card: gtk::Widget,
    notes: gtk::Label,
    progress: gtk::ProgressBar,
    secondary: gtk::Button,
    primary: gtk::Button,
    /// Stops a download when the dialog closes
    cancellable: gio::Cancellable,
}

/// Opens the dialog over `parent`, checking GitHub first when needed
pub fn present(parent: &impl IsA<gtk::Widget>) {
    let parts = build();
    parts.dialog.present(Some(parent));

    match LpApplication::default().update() {
        Some(release) => show_release(&parts, release),
        None => check(&parts),
    }
}

fn build() -> Parts {
    let title = gtk::Label::builder()
        .css_classes(["title-2"])
        .wrap(true)
        .justify(gtk::Justification::Center)
        .build();
    let subtitle = gtk::Label::builder()
        .css_classes(["dim-label"])
        .wrap(true)
        .justify(gtk::Justification::Center)
        .build();

    let notes = gtk::Label::builder()
        .wrap(true)
        .xalign(0.)
        .yalign(0.)
        .selectable(true)
        .build();
    let heading = gtk::Label::builder()
        .label(gettext("What’s New"))
        .css_classes(["heading"])
        .xalign(0.)
        .build();
    let notes_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(6)
        .margin_top(12)
        .margin_bottom(12)
        .margin_start(12)
        .margin_end(12)
        .build();
    notes_box.append(&heading);
    notes_box.append(&notes);
    let scrolled = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .propagate_natural_height(true)
        .max_content_height(260)
        .child(&notes_box)
        .build();
    let notes_card = gtk::Frame::builder()
        .css_classes(["view"])
        .child(&scrolled)
        .visible(false)
        .build();

    let progress = gtk::ProgressBar::builder().visible(false).build();

    let secondary = gtk::Button::builder().build();
    let primary = gtk::Button::builder()
        .css_classes(["suggested-action"])
        .build();
    let buttons = gtk::Box::builder()
        .spacing(8)
        .halign(gtk::Align::End)
        .margin_top(6)
        .build();
    buttons.append(&secondary);
    buttons.append(&primary);

    let content = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_start(24)
        .margin_end(24)
        .margin_bottom(24)
        .build();
    content.append(&title);
    content.append(&subtitle);
    content.append(&notes_card);
    content.append(&progress);
    content.append(&buttons);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::builder().show_title(false).build());
    toolbar.set_content(Some(&content));

    let dialog = adw::Dialog::builder()
        .content_width(380)
        .child(&toolbar)
        .build();

    let parts = Parts {
        dialog,
        title,
        subtitle,
        notes_card: notes_card.upcast(),
        notes,
        progress,
        secondary,
        primary,
        cancellable: gio::Cancellable::new(),
    };

    // The button handlers hold the parts, and with them the dialog. Taking
    // the dialog apart once closed breaks that loop, so it is freed.
    parts.dialog.connect_closed(clone!(
        #[strong(rename_to = cancellable)]
        parts.cancellable,
        move |dialog| {
            cancellable.cancel();
            let dialog = dialog.clone();
            glib::idle_add_local_once(move || {
                // SAFETY: The dialog is no longer shown and its handlers can't
                // run anymore. A running check only sets labels, which stays
                // harmless after dispose.
                unsafe { dialog.run_dispose() }
            });
        }
    ));

    parts.secondary.connect_clicked(clone!(
        #[weak(rename_to = dialog)]
        parts.dialog,
        move |_| {
            dialog.close();
        }
    ));

    parts
}

/// Replaces what the primary button does
fn set_primary(parts: &Parts, label: &str, action: impl Fn(&Parts) + 'static) {
    let primary = &parts.primary;
    // A fresh button drops the previous handler
    let button = gtk::Button::builder()
        .label(label)
        .css_classes(["suggested-action"])
        .build();
    let parent = primary.parent().and_downcast::<gtk::Box>().unwrap();
    parent.insert_child_after(&button, Some(primary));
    parent.remove(primary);

    let mut new_parts = parts.clone();
    new_parts.primary = button.clone();
    let parts = new_parts;
    button.connect_clicked(move |_| action(&parts));
    button.grab_focus();
}

fn check(parts: &Parts) {
    parts.title.set_label(&gettext("Checking for Updates…"));
    parts.subtitle.set_label("");
    parts.secondary.set_label(&gettext("Close"));
    parts.primary.set_visible(false);

    let parts = parts.clone();
    glib::spawn_future_local(async move {
        match LpApplication::default().check_for_updates().await {
            Ok(Some(release)) => show_release(&parts, release),
            Ok(None) => show_up_to_date(&parts),
            Err(err) => {
                tracing::warn!("Could not check for updates: {err:#}");
                show_error(&parts);
            }
        }
    });
}

fn show_up_to_date(parts: &Parts) {
    parts.title.set_label(&gettext("Glance Is Up to Date"));
    // Translators: {} is replaced with a version number
    parts
        .subtitle
        .set_label(&gettext_f("Version {}", [config::VERSION]));
    parts.secondary.set_label(&gettext("Close"));
    parts.primary.set_visible(false);
}

fn show_error(parts: &Parts) {
    parts
        .title
        .set_label(&gettext("Could Not Check for Updates"));
    parts
        .subtitle
        .set_label(&gettext("Check your internet connection and try again."));
    parts.secondary.set_label(&gettext("Close"));
    parts.primary.set_visible(true);
    set_primary(parts, &gettext("Try Again"), check);
}

fn show_release(parts: &Parts, release: Release) {
    // Translators: {} is replaced with a version number
    parts
        .title
        .set_label(&gettext_f("Glance {} Is Available", [&release.version]));
    // Translators: {} is replaced with a version number
    parts
        .subtitle
        .set_label(&gettext_f("You have version {}", [config::VERSION]));
    parts.notes.set_label(&release.notes);
    parts.notes_card.set_visible(!release.notes.is_empty());
    parts.secondary.set_label(&gettext("Later"));
    parts.primary.set_visible(true);

    let kind = InstallKind::detect();
    if release.asset_for(&kind).is_some() {
        set_primary(parts, &gettext("Update"), move |parts| {
            install(parts, release.clone(), kind.clone())
        });
    } else {
        set_primary(parts, &gettext("Open Download Page"), move |parts| {
            gtk::UriLauncher::new(&release.page).launch(
                parts.dialog.root().and_downcast::<gtk::Window>().as_ref(),
                gio::Cancellable::NONE,
                |_| {},
            );
            parts.dialog.close();
        });
    }
}

fn install(parts: &Parts, release: Release, kind: InstallKind) {
    let Some(asset) = release.asset_for(&kind).cloned() else {
        return;
    };

    parts.subtitle.set_label(&gettext("Downloading…"));
    parts.progress.set_fraction(0.);
    parts.progress.set_visible(true);
    parts.primary.set_sensitive(false);

    let parts = parts.clone();
    glib::spawn_future_local(async move {
        let progress = parts.progress.clone();
        let result = crate::updates::download(
            &release,
            &asset,
            &kind,
            &parts.cancellable,
            |fraction| progress.set_fraction(fraction),
        )
        .await;
        // Nothing is installed after the dialog was closed
        if parts.cancellable.is_cancelled() {
            return;
        }
        parts.progress.set_visible(false);
        parts.primary.set_sensitive(true);

        let file = match result {
            Ok(file) => file,
            Err(err) => {
                tracing::warn!("Could not download the update: {err:#}");
                parts.subtitle.set_label(&gettext("The download failed."));
                set_primary(&parts, &gettext("Try Again"), move |parts| {
                    install(parts, release.clone(), kind.clone())
                });
                return;
            }
        };

        if let InstallKind::AppImage(path) = &kind {
            // The AppImage was replaced, starting it again runs the new version
            let path = path.clone();
            parts
                .subtitle
                .set_label(&gettext("Restart Glance to use the new version."));
            set_primary(&parts, &gettext("Restart Glance"), move |parts| {
                parts.dialog.close();
                restart(path.clone());
            });
            return;
        }

        // The system's software installer takes over
        gtk::FileLauncher::new(Some(&file)).launch(
            parts.dialog.root().and_downcast::<gtk::Window>().as_ref(),
            gio::Cancellable::NONE,
            |result| {
                if let Err(err) = result {
                    tracing::warn!("Could not open the update: {err}");
                }
            },
        );
        parts
            .subtitle
            .set_label(&gettext("Finish installing in the window that opened."));
        parts.secondary.set_label(&gettext("Close"));
        parts.primary.set_visible(false);
    });
}

/// Closes all windows, asking about unsaved edits, and starts `path` once
/// Glance has quit
///
/// The new copy starts only after this one gave up its name on the session
/// bus, as it would otherwise hand over to this copy and exit.
fn restart(path: std::path::PathBuf) {
    let app = LpApplication::default();
    app.connect_shutdown(move |_| {
        let result = std::process::Command::new("sh")
            .arg("-c")
            .arg(r#"sleep 1; exec "$0""#)
            .arg(&path)
            .spawn();
        if let Err(err) = result {
            tracing::warn!("Could not restart Glance: {err}");
        }
    });
    app.activate_action("quit", None);
}
