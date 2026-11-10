// Copyright (c) 2024 Sophie Herold
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

pub const APP_ID: &str = default_env(option_env!("APP_ID"), "io.github.Replexo.Glance.Devel");
pub const VERSION: &str = default_env(option_env!("VERSION"), "unknown");
pub const LOCALEDIR: &str = default_env(option_env!("LOCALEDIR"), "/usr/share/locale/");
/// Whether Glance looks for new releases on GitHub, off for Flathub and distributions
pub const UPDATE_CHECK: bool = is_true(option_env!("UPDATE_CHECK"));

const fn default_env(v: Option<&'static str>, default: &'static str) -> &'static str {
    match v {
        Some(v) => v,
        None => default,
    }
}

const fn is_true(v: Option<&'static str>) -> bool {
    matches!(v, Some(v) if v.len() == 4 && v.as_bytes()[0] == b't')
}
