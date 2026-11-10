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

//! Looks for new releases on GitHub and installs them
//!
//! Only builds with the `update_check` option do this. Flathub and
//! distribution packages are updated by the system instead.

use std::path::{Path, PathBuf};

use gtk::prelude::*;
use soup::prelude::*;

use crate::config;
use crate::deps::*;

const LATEST_RELEASE: &str = "https://api.github.com/repos/Replexo/Glance/releases/latest";

/// A release published on GitHub
#[derive(Debug, Clone)]
pub struct Release {
    pub version: String,
    pub notes: String,
    pub page: String,
    pub assets: Vec<Asset>,
}

#[derive(Debug, Clone)]
pub struct Asset {
    pub name: String,
    pub url: String,
}

impl Release {
    pub fn is_newer(&self) -> bool {
        version_parts(&self.version) > version_parts(config::VERSION)
    }

    /// The download that fits how Glance is installed
    pub fn asset_for(&self, kind: &InstallKind) -> Option<&Asset> {
        let suffix = kind.suffix()?;
        let arch_names: &[&str] = match std::env::consts::ARCH {
            "aarch64" => &["aarch64", "arm64"],
            _ => &["x86_64", "amd64"],
        };

        let matching: Vec<_> = self
            .assets
            .iter()
            .filter(|asset| asset.name.ends_with(suffix))
            .collect();
        // A lone download without an architecture in its name fits all
        const ALL_ARCHS: &[&str] = &["x86_64", "amd64", "aarch64", "arm64", "i686", "armhf"];
        matching
            .iter()
            .find(|asset| arch_names.iter().any(|arch| asset.name.contains(arch)))
            .or_else(|| {
                (matching.len() == 1
                    && !ALL_ARCHS.iter().any(|arch| matching[0].name.contains(arch)))
                .then(|| &matching[0])
            })
            .copied()
    }

    /// The list of SHA-256 checksums published with the release, if any
    pub fn checksums(&self) -> Option<&Asset> {
        self.assets.iter().find(|asset| asset.name == "SHA256SUMS")
    }
}

/// How this copy of Glance was installed
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InstallKind {
    Flatpak,
    AppImage(PathBuf),
    Deb,
    Rpm,
    /// Arch and others are updated through their package manager
    Other,
}

impl InstallKind {
    pub fn detect() -> Self {
        if Path::new("/.flatpak-info").exists() {
            return Self::Flatpak;
        }
        if let Some(path) = std::env::var_os("APPIMAGE") {
            return Self::AppImage(path.into());
        }

        let os_release = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
        let ids: Vec<&str> = os_release
            .lines()
            .filter_map(|line| {
                line.strip_prefix("ID=")
                    .or_else(|| line.strip_prefix("ID_LIKE="))
            })
            .flat_map(|value| value.trim_matches('"').split_whitespace())
            .collect();
        let has = |names: &[&str]| ids.iter().any(|id| names.contains(id));

        if has(&["debian", "ubuntu"]) {
            Self::Deb
        } else if has(&["fedora", "rhel", "suse", "opensuse"]) {
            Self::Rpm
        } else {
            Self::Other
        }
    }

    fn suffix(&self) -> Option<&'static str> {
        match self {
            Self::Flatpak => Some(".flatpak"),
            Self::AppImage(_) => Some(".AppImage"),
            Self::Deb => Some(".deb"),
            Self::Rpm => Some(".rpm"),
            Self::Other => None,
        }
    }
}

/// Numbers of a version like `v1.10.2`, for comparing
fn version_parts(version: &str) -> Vec<u64> {
    version
        .trim_start_matches(['v', 'V'])
        .split(['.', '-', '+'])
        .map_while(|part| part.parse().ok())
        .collect()
}

fn session() -> soup::Session {
    let session = soup::Session::new();
    session.set_user_agent(&format!("Glance/{}", config::VERSION));
    session
}

/// Asks GitHub for the latest release
pub async fn latest_release() -> anyhow::Result<Release> {
    let message = soup::Message::new("GET", LATEST_RELEASE)?;
    message
        .request_headers()
        .ok_or_else(|| anyhow::anyhow!("No request headers"))?
        .append("Accept", "application/vnd.github+json");

    let bytes = session()
        .send_and_read_future(&message, glib::Priority::DEFAULT)
        .await?;
    if message.status() != soup::Status::Ok {
        anyhow::bail!("GitHub answered {:?}", message.status());
    }

    let json: serde_json::Value = serde_json::from_slice(&bytes)?;
    let text = |key: &str| json[key].as_str().unwrap_or_default().to_string();

    let assets = json["assets"]
        .as_array()
        .map(|assets| {
            assets
                .iter()
                .filter_map(|asset| {
                    Some(Asset {
                        name: asset["name"].as_str()?.to_string(),
                        url: asset["browser_download_url"].as_str()?.to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(Release {
        version: text("tag_name").trim_start_matches(['v', 'V']).to_string(),
        notes: plain_notes(&text("body")),
        page: text("html_url"),
        assets,
    })
}

/// Turns the Markdown of release notes into plain lines with bullets
fn plain_notes(markdown: &str) -> String {
    markdown
        .lines()
        .map(|line| {
            let line = line.trim().replace("**", "").replace('`', "");
            if let Some(item) = line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
                format!("• {item}")
            } else {
                line.trim_start_matches('#').trim().to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// Downloads `asset` and reports progress from 0 to 1
///
/// The file goes to the cache, or next to the AppImage it replaces. When
/// the release lists checksums, a download that does not match is dropped.
pub async fn download(
    release: &Release,
    asset: &Asset,
    kind: &InstallKind,
    cancellable: &gio::Cancellable,
    progress: impl Fn(f64),
) -> anyhow::Result<gio::File> {
    let expected = match release.checksums() {
        Some(sums) => Some(expected_checksum(sums, &asset.name, cancellable).await?),
        None => None,
    };

    let target = match kind {
        InstallKind::AppImage(path) => path
            .parent()
            .ok_or_else(|| anyhow::anyhow!("AppImage has no folder"))?
            .join(format!(".{}.part", asset.name)),
        _ => {
            let dir = glib::user_cache_dir().join("glance-updates");
            std::fs::create_dir_all(&dir)?;
            dir.join(&asset.name)
        }
    };
    let file = gio::File::for_path(&target);

    let message = soup::Message::new("GET", &asset.url)?;
    let input = gio::CancellableFuture::new(
        session().send_future(&message, glib::Priority::DEFAULT),
        cancellable.clone(),
    )
    .await??;
    if message.status() != soup::Status::Ok {
        anyhow::bail!("Download answered {:?}", message.status());
    }
    let total = message
        .response_headers()
        .map(|headers| headers.content_length())
        .unwrap_or_default();

    let output = file
        .replace_future(
            None,
            false,
            gio::FileCreateFlags::REPLACE_DESTINATION,
            glib::Priority::DEFAULT,
        )
        .await?;
    let mut done = 0;
    let mut checksum = glib::Checksum::new(glib::ChecksumType::Sha256)
        .ok_or_else(|| anyhow::anyhow!("No SHA-256 support"))?;
    loop {
        if cancellable.is_cancelled() {
            let _ = output.close_future(glib::Priority::DEFAULT).await;
            let _ = std::fs::remove_file(&target);
            anyhow::bail!("Download cancelled");
        }
        let chunk = input
            .read_bytes_future(64 * 1024, glib::Priority::DEFAULT)
            .await?;
        if chunk.is_empty() {
            break;
        }
        checksum.update(&chunk);
        output
            .write_all_future(chunk.to_vec(), glib::Priority::DEFAULT)
            .await
            .map_err(|(_, err)| err)?;
        done += chunk.len() as i64;
        if total > 0 {
            progress(done as f64 / total as f64);
        }
    }
    output.close_future(glib::Priority::DEFAULT).await?;

    if let Some(expected) = expected {
        if checksum.string().as_deref() != Some(expected.as_str()) {
            let _ = std::fs::remove_file(&target);
            anyhow::bail!("Checksum of {} does not match", asset.name);
        }
    }

    if let InstallKind::AppImage(path) = kind {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755))?;
        std::fs::rename(&target, path)?;
        return Ok(gio::File::for_path(path));
    }

    Ok(file)
}

/// The SHA-256 checksum listed for `name` in the release's `SHA256SUMS`
async fn expected_checksum(
    sums: &Asset,
    name: &str,
    cancellable: &gio::Cancellable,
) -> anyhow::Result<String> {
    let message = soup::Message::new("GET", &sums.url)?;
    let bytes = gio::CancellableFuture::new(
        session().send_and_read_future(&message, glib::Priority::DEFAULT),
        cancellable.clone(),
    )
    .await??;
    if message.status() != soup::Status::Ok {
        anyhow::bail!("Checksums answered {:?}", message.status());
    }

    // Lines look like `<hex>  <name>` or `<hex> *<name>`
    String::from_utf8_lossy(&bytes)
        .lines()
        .filter_map(|line| line.split_once(char::is_whitespace))
        .find(|(_, file)| file.trim_start().trim_start_matches('*') == name)
        .map(|(sum, _)| sum.to_ascii_lowercase())
        .ok_or_else(|| anyhow::anyhow!("No checksum listed for {name}"))
}
