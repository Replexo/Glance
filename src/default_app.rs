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

//! Makes Glance the default app for the image types it opens
//!
//! Writes the "Default Applications" group of `mimeapps.list`, which GNOME and
//! KDE both read. Inside Flatpak, GIO would only change the sandbox's own copy,
//! so the host's file is written directly.

use std::path::{Path, PathBuf};

use anyhow::Context;
use gio::prelude::*;

use crate::config;
use crate::deps::*;

const DEFAULTS: &str = "Default Applications";
const REMOVED: &str = "Removed Associations";

fn desktop_id() -> String {
    format!("{}.desktop", config::APP_ID)
}

fn is_flatpak() -> bool {
    Path::new("/.flatpak-info").exists()
}

/// The host's `mimeapps.list`, with symlinks resolved so managed dotfiles
/// are written through instead of replaced
pub fn mimeapps_path() -> PathBuf {
    let dir = if is_flatpak() {
        std::env::var_os("HOST_XDG_CONFIG_HOME")
            .filter(|x| !x.is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| glib::home_dir().join(".config"))
    } else {
        glib::user_config_dir()
    };
    let path = dir.join("mimeapps.list");
    std::fs::canonicalize(&path).unwrap_or(path)
}

fn app_info() -> Option<gio::AppInfo> {
    let id = desktop_id();
    gio::AppInfo::all()
        .into_iter()
        .find(|app| app.id().is_some_and(|x| x == id))
}

/// Whether the desktop file is installed, which dev builds may lack
pub fn is_available() -> bool {
    app_info().is_some()
}

/// Image types listed in the installed desktop file
fn mime_types() -> anyhow::Result<Vec<String>> {
    let app_info = app_info().context("The desktop file of the app is not installed")?;
    Ok(app_info
        .supported_types()
        .iter()
        .map(|x| x.to_string())
        .filter(|x| x.starts_with("image/"))
        .collect())
}

fn load(path: &Path) -> anyhow::Result<glib::KeyFile> {
    let key_file = glib::KeyFile::new();
    if path.exists() {
        key_file.load_from_file(path, glib::KeyFileFlags::KEEP_COMMENTS)?;
    }
    Ok(key_file)
}

/// `apps`, a `;`-separated list, without `id`
fn without(apps: &str, id: &str) -> String {
    apps.split(';')
        .filter(|app| !app.is_empty() && *app != id)
        .map(|app| format!("{app};"))
        .collect()
}

/// Whether Glance already opens JPEG and PNG images by default
pub fn is_default() -> bool {
    let id = desktop_id();
    let types = ["image/jpeg", "image/png"];
    // Natively GIO knows, including desktop-specific lists
    if !is_flatpak() {
        return types.iter().all(|mime| {
            gio::AppInfo::default_for_type(mime, false)
                .and_then(|app| app.id())
                .is_some_and(|x| x == id)
        });
    }
    let Ok(key_file) = load(&mimeapps_path()) else {
        return false;
    };
    types.iter().all(|mime| {
        key_file
            .string(DEFAULTS, mime)
            .is_ok_and(|apps| apps.split(';').next() == Some(id.as_str()))
    })
}

/// Sets Glance as the default for every image type it opens, and returns
/// how many types changed
pub fn make_default() -> anyhow::Result<usize> {
    let types = mime_types()?;
    let path = mimeapps_path();
    let key_file = load(&path)?;
    let id = desktop_id();

    let mut changed = 0;
    for mime in &types {
        // Other apps stay as fallbacks after Glance
        let others = key_file
            .string(DEFAULTS, mime)
            .map(|apps| without(&apps, &id))
            .unwrap_or_default();
        let value = format!("{id};{others}");
        if key_file.string(DEFAULTS, mime).ok().as_deref() != Some(value.as_str()) {
            key_file.set_string(DEFAULTS, mime, &value);
            changed += 1;
        }
        // A removed association would hide the new default
        if let Ok(removed) = key_file.string(REMOVED, mime) {
            let rest = without(&removed, &id);
            if rest != removed.as_str() {
                if rest.is_empty() {
                    let _ = key_file.remove_key(REMOVED, mime);
                } else {
                    key_file.set_string(REMOVED, mime, &rest);
                }
            }
        }
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    key_file.save_to_file(&path)?;
    Ok(changed)
}
