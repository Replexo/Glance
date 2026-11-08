// Copyright (c) 2020-2023 Christopher Davis
// Copyright (c) 2022-2026 Sophie Herold
// Copyright (c) 2023 Matteo Nardi
// Copyright (c) 2023 Julian Hofer
// Copyright (c) 2023 Huan Nguyen
// Copyright (c) 2024 Balló György
// Copyright (c) 2025 Abhinandan Thakur
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

use adw::prelude::*;
use adw::subclass::prelude::*;

use gettextrs::gettext;

use crate::config;
use crate::deps::*;
use crate::widgets::LpWindow;

mod imp {
    use super::*;
    use crate::widgets::LpShyBin;

    // The basic struct that holds our
    // state and widgets
    #[derive(Debug)]
    pub struct LpApplication {
        pub settings: gio::Settings,
        /// Watches the default apps for the menu item
        pub defaults_monitor: std::cell::RefCell<Option<gio::FileMonitor>>,
    }

    impl Default for LpApplication {
        fn default() -> Self {
            Self {
                settings: gio::Settings::new(config::APP_ID),
                defaults_monitor: Default::default(),
            }
        }
    }

    // Sets up the basics for the GObject
    // The `#[glib::object_subclass] macro implements
    // some boilerplate code for the object setup, e.g. get_type()
    #[glib::object_subclass]
    impl ObjectSubclass for LpApplication {
        const NAME: &'static str = "LpApplication";
        type Type = super::LpApplication;
        type ParentType = adw::Application;
    }

    impl ObjectImpl for LpApplication {
        fn constructed(&self) {
            let obj = self.obj();

            self.parent_constructed();

            // Set up the actions
            obj.setup_actions();
        }
    }

    impl ApplicationImpl for LpApplication {
        fn startup(&self) {
            tracing::trace!("Startup");
            self.parent_startup();

            // Force dark theme
            self.obj()
                .style_manager()
                .set_color_scheme(adw::ColorScheme::PreferDark);

            LpShyBin::ensure_type();
            gtk::Window::set_default_icon_name(config::APP_ID);
        }

        fn activate(&self) {
            tracing::debug!("Showing window via 'activate'");
            let application = self.obj();
            let window = LpWindow::new(&*application);
            window.present();
        }

        // Handles opening files from the command line or other applications
        fn open(&self, files: &[gio::File], _hint: &str) {
            tracing::debug!("Open {} file(s)", files.len());
            let application = self.obj();
            let win = LpWindow::new(&*application);
            win.image_view().set_images_from_files(files.to_vec());
        }
    }

    // This is empty, but we still need to provide an
    // empty implementation for each type we subclass.
    impl GtkApplicationImpl for LpApplication {}
    impl AdwApplicationImpl for LpApplication {}
}

// Creates a wrapper struct that inherits the functions
// from objects listed as @extends or interfaces it @implements.
// This is what allows us to do e.g. application.quit() on
// LpApplication without casting.
glib::wrapper! {
    pub struct LpApplication(ObjectSubclass<imp::LpApplication>)
        @extends gio::Application, gtk::Application, adw::Application,
        @implements gio::ActionGroup, gio::ActionMap;
}

// This is where the member functions of LpApplication go.
#[allow(clippy::new_without_default)]
impl LpApplication {
    pub fn new() -> Self {
        glib::Object::builder()
            .property("application-id", config::APP_ID)
            .property("flags", gio::ApplicationFlags::HANDLES_OPEN)
            .property("resource-base-path", "/io/github/Replexo/Glance")
            .build()
    }

    pub fn setup_actions(&self) {
        // gio::ActionEntryBuilder allows us to build and store an action on an object
        // that implements gio::ActionMap. Here we build the application's actions and
        // add them with add_action_entries().
        let actions = [
            gio::ActionEntryBuilder::new("help")
                .activate(|app: &Self, _, _| app.show_help())
                .build(),
            gio::ActionEntryBuilder::new("quit")
                .activate(|app: &Self, _, _| {
                    // Give all windows the chance to handle close-request
                    for win in app.windows() {
                        win.close();
                    }
                })
                .build(),
            gio::ActionEntryBuilder::new("make-default")
                .activate(|app: &Self, _, _| app.make_default())
                .build(),
            gio::ActionEntryBuilder::new("new-window")
                .activate(|app: &Self, _, _| {
                    let win = LpWindow::new(app);
                    tracing::debug!("Showing new window");
                    win.present();
                })
                .build(),
        ];

        self.add_action_entries(actions);
        self.update_make_default();
        self.watch_defaults();

        self.set_accels_for_action("app.help", &["F1"]);
        self.set_accels_for_action("app.quit", &["<Ctrl>Q"]);
        self.set_accels_for_action("window.close", &["<Ctrl>W"]);
        self.set_accels_for_action("app.new-window", &["<Ctrl>N"]);
    }

    /// Makes Glance the default image viewer, after the user asked for it
    fn make_default(&self) {
        let window = self.active_window().and_downcast::<LpWindow>();
        match crate::default_app::make_default() {
            Ok(_) => {
                if let Some(window) = window {
                    window.show_toast(
                        &gettext("Glance is now the default image viewer"),
                        adw::ToastPriority::High,
                    );
                }
            }
            Err(err) => {
                tracing::error!("Failed to set the default image viewer: {err:#}");
                if let Some(window) = window {
                    window.show_toast(
                        &gettext("Could not make Glance the default image viewer"),
                        adw::ToastPriority::High,
                    );
                }
            }
        }
        self.update_make_default();
    }

    /// Greys out the menu item while Glance is the default already, or
    /// when the desktop file is not installed
    fn update_make_default(&self) {
        if let Some(action) = self
            .lookup_action("make-default")
            .and_downcast::<gio::SimpleAction>()
        {
            action.set_enabled(
                crate::default_app::is_available() && !crate::default_app::is_default(),
            );
        }
    }

    /// Follows default apps changed elsewhere, like in the system settings
    fn watch_defaults(&self) {
        let file = gio::File::for_path(crate::default_app::mimeapps_path());
        match file.monitor_file(gio::FileMonitorFlags::NONE, gio::Cancellable::NONE) {
            Ok(monitor) => {
                monitor.connect_changed(glib::clone!(
                    #[weak(rename_to = app)]
                    self,
                    move |_, _, _, _| app.update_make_default()
                ));
                self.imp().defaults_monitor.replace(Some(monitor));
            }
            Err(err) => tracing::debug!("Not watching default apps: {err}"),
        }
    }

    pub fn show_help(&self) {
        let context = self
            .active_window()
            .map(|w| gtk::prelude::WidgetExt::display(&w).app_launch_context());
        glib::spawn_future_local(async move {
            if let Err(e) =
                gio::AppInfo::launch_default_for_uri_future("help:glance", context.as_ref()).await
            {
                tracing::error!("Failed to launch help: {}", e.message());
            }
        });
    }

    pub fn settings(&self) -> gio::Settings {
        self.imp().settings.clone()
    }
}

impl Default for LpApplication {
    fn default() -> Self {
        gio::Application::default()
            .unwrap()
            .downcast::<LpApplication>()
            .unwrap()
    }
}
