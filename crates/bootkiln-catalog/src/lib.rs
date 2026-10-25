//! The OS catalog: which operating systems bootkiln can download, how to find each one's
//! latest release on its official servers, and a resumable, checksum-verified downloader.
//!
//! Every entry has a resolver that reads the distro's own release index at run time, so
//! there is no catalog file to host or update. If a distro changes its site, only that
//! entry fails.

mod download;
mod http;
mod linux;
mod windows;

use serde::Serialize;

pub use download::{Stage, discard_partial, download};
pub use windows::{WINDOWS_LANGUAGE_FALLBACK, default_windows_language};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    Http(String),
    #[error("the release index has an unexpected format: {0}")]
    Format(String),
    #[error("Microsoft refused the download: {0}")]
    MicrosoftRefused(String),
    #[error("{file} does not match its published SHA-256 checksum; the download was deleted")]
    ChecksumMismatch { file: String },
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// The download was stopped and its partial file deleted.
    #[error("download cancelled")]
    Cancelled,
    /// The download was stopped; its partial file is kept for resuming.
    #[error("download paused")]
    Paused,
}

/// One downloadable operating system.
#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Entry {
    pub id: &'static str,
    pub name: &'static str,
    pub summary: &'static str,
    /// Brand colour for the tile, `#RRGGBB`.
    pub color: &'static str,
    /// Windows needs a language choice.
    pub windows: bool,
}

pub const ENTRIES: &[Entry] = &[
    Entry { id: "ubuntu", name: "Ubuntu", summary: "Desktop · latest LTS", color: "#E95420", windows: false },
    Entry { id: "kubuntu", name: "Kubuntu", summary: "KDE Plasma · latest LTS", color: "#0079C1", windows: false },
    Entry { id: "fedora-workstation", name: "Fedora Workstation", summary: "GNOME", color: "#51A2DA", windows: false },
    Entry { id: "fedora-kde", name: "Fedora KDE Plasma", summary: "KDE Plasma", color: "#294172", windows: false },
    Entry { id: "linuxmint", name: "Linux Mint", summary: "Cinnamon", color: "#69A83D", windows: false },
    Entry { id: "debian", name: "Debian", summary: "Network installer", color: "#A80030", windows: false },
    Entry { id: "debian-live-kde", name: "Debian Live", summary: "KDE Plasma", color: "#D70751", windows: false },
    Entry { id: "arch", name: "Arch Linux", summary: "Installer", color: "#1793D1", windows: false },
    Entry { id: "windows11", name: "Windows 11", summary: "x64 · Home, Pro, Education", color: "#0078D4", windows: true },
];

/// A concrete file to download.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Release {
    pub entry: String,
    pub version: String,
    pub file_name: String,
    pub url: String,
    /// Lowercase hex SHA-256 published by the vendor.
    pub sha256: String,
    pub size: Option<u64>,
    /// Windows: the language of this file and all languages on offer.
    pub language: Option<String>,
    pub languages: Vec<String>,
}

/// Find the latest release of entry `id` on its official servers. `language` picks the
/// Windows language (a name from [`Release::languages`]); `None` means English.
pub fn resolve(id: &str, language: Option<&str>) -> Result<Release, Error> {
    match id {
        "ubuntu" => linux::ubuntu(),
        "kubuntu" => linux::kubuntu(),
        "fedora-workstation" => linux::fedora(id, "Workstation"),
        "fedora-kde" => linux::fedora(id, "KDE"),
        "linuxmint" => linux::linuxmint(),
        "debian" => linux::debian_netinst(),
        "debian-live-kde" => linux::debian_live_kde(),
        "arch" => linux::arch(),
        "windows11" => windows::windows11(language.unwrap_or(WINDOWS_LANGUAGE_FALLBACK)),
        _ => Err(Error::Format(format!("unknown catalog entry {id}"))),
    }
}

/// `"<hash> [*]<file>"` lines of a SHA256SUMS-style file: the hash for `file`.
pub(crate) fn checksum_for(sums: &str, file: &str) -> Option<String> {
    sums.lines().find_map(|line| {
        let (hash, name) = line.trim().split_once(char::is_whitespace)?;
        (name.trim().trim_start_matches('*') == file && hash.len() == 64 && hash.bytes().all(|b| b.is_ascii_hexdigit()))
            .then(|| hash.to_ascii_lowercase())
    })
}

/// `downloads` next to the running executable: bootkiln is portable, so its downloads live
/// with it.
pub fn default_download_dir() -> std::io::Result<std::path::PathBuf> {
    let exe = std::env::current_exe()?;
    let dir = exe.parent().ok_or_else(|| std::io::Error::other("the executable has no folder"))?;
    Ok(dir.join("downloads"))
}

/// Dotted numeric versions ("26.04.1") as comparable vectors; `None` for anything else.
pub(crate) fn version_key(version: &str) -> Option<Vec<u32>> {
    version.split('.').map(|part| part.parse().ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_lines_in_both_styles() {
        let sums = "AA11  other.iso\n\
                    95ce9cf68f13015b9a88bd1ef86fcf7eda77c99979fda48c69e28aa0a84f88ac *kubuntu-26.04-desktop-amd64.iso\n\
                    684ded26c63240ff4a41e8c25ee84ea6da233f557364821f13d12c2b0a9059a5  archlinux-x86_64.iso\n";
        assert_eq!(
            checksum_for(sums, "kubuntu-26.04-desktop-amd64.iso").as_deref(),
            Some("95ce9cf68f13015b9a88bd1ef86fcf7eda77c99979fda48c69e28aa0a84f88ac")
        );
        assert!(checksum_for(sums, "archlinux-x86_64.iso").is_some());
        assert_eq!(checksum_for(sums, "other.iso"), None, "not a SHA-256");
        assert_eq!(checksum_for(sums, "kubuntu-26.04-desktop-amd64.iso.zsync"), None);
    }

    #[test]
    fn versions_compare_numerically() {
        assert!(version_key("26.04.1") > version_key("26.04"));
        assert!(version_key("22.10") > version_key("22.3"));
        assert_eq!(version_key("45 Beta"), None);
    }
}
