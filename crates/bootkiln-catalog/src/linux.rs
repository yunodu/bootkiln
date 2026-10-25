//! Resolvers for Linux distributions, each reading the distro's official release index.

use regex_lite::Regex;
use serde::Deserialize;

use crate::{Error, Release, checksum_for, http, version_key};

const UBUNTU: &str = "https://releases.ubuntu.com";
const KUBUNTU: &str = "https://cdimage.ubuntu.com/kubuntu/releases";
const FEDORA_RELEASES: &str = "https://fedoraproject.org/releases.json";
const MINT: &str = "https://mirrors.edge.kernel.org/linuxmint/stable";
const DEBIAN_NETINST: &str = "https://cdimage.debian.org/debian-cd/current/amd64/iso-cd";
const DEBIAN_LIVE: &str = "https://cdimage.debian.org/debian-cd/current-live/amd64/iso-hybrid";
const ARCH: &str = "https://geo.mirror.pkgbuild.com/iso/latest";

pub(crate) fn ubuntu() -> Result<Release, Error> {
    let version = latest_lts(&http::text(&format!("{UBUNTU}/"), &[])?)?;
    let file = format!("ubuntu-{version}-desktop-amd64.iso");
    let sums = http::text(&format!("{UBUNTU}/{version}/SHA256SUMS"), &[])?;
    release("ubuntu", &version, file.clone(), format!("{UBUNTU}/{version}/{file}"), &sums)
}

pub(crate) fn kubuntu() -> Result<Release, Error> {
    let version = latest_lts(&http::text(&format!("{KUBUNTU}/"), &[])?)?;
    let file = format!("kubuntu-{version}-desktop-amd64.iso");
    let sums = http::text(&format!("{KUBUNTU}/{version}/release/SHA256SUMS"), &[])?;
    release("kubuntu", &version, file.clone(), format!("{KUBUNTU}/{version}/release/{file}"), &sums)
}

/// Highest `YY.04[.N]` directory with an even year: Ubuntu's LTS releases.
fn latest_lts(index: &str) -> Result<String, Error> {
    let dirs = Regex::new(r#"href="(\d{2})\.04((?:\.\d+)?)/""#).expect("valid regex");
    dirs.captures_iter(index)
        .filter(|c| c[1].parse::<u32>().is_ok_and(|year| year % 2 == 0))
        .map(|c| format!("{}.04{}", &c[1], &c[2]))
        .max_by_key(|v| version_key(v))
        .ok_or_else(|| Error::Format("no LTS release listed".into()))
}

#[derive(Deserialize)]
struct FedoraRelease {
    version: String,
    arch: String,
    subvariant: String,
    link: String,
    sha256: Option<String>,
    size: Option<String>,
}

pub(crate) fn fedora(entry: &str, subvariant: &str) -> Result<Release, Error> {
    let releases: Vec<FedoraRelease> = serde_json::from_str(&http::text(FEDORA_RELEASES, &[])?)
        .map_err(|e| Error::Format(format!("{FEDORA_RELEASES}: {e}")))?;
    pick_fedora(entry, subvariant, releases)
}

fn pick_fedora(entry: &str, subvariant: &str, releases: Vec<FedoraRelease>) -> Result<Release, Error> {
    // Betas have versions like "45 Beta", which do not parse as numbers.
    let latest = releases
        .into_iter()
        .filter(|r| r.arch == "x86_64" && r.subvariant == subvariant && r.link.ends_with(".iso"))
        .filter(|r| version_key(&r.version).is_some())
        .max_by_key(|r| version_key(&r.version))
        .ok_or_else(|| Error::Format(format!("no Fedora {subvariant} release listed")))?;
    let sha256 = latest
        .sha256
        .filter(|h| h.len() == 64 && h.bytes().all(|b| b.is_ascii_hexdigit()))
        .ok_or_else(|| Error::Format(format!("Fedora {subvariant} {} has no SHA-256", latest.version)))?;
    let file_name = latest.link.rsplit('/').next().unwrap_or_default().to_owned();
    Ok(Release {
        entry: entry.to_owned(),
        version: latest.version,
        file_name,
        size: latest.size.and_then(|s| s.parse().ok()),
        url: latest.link,
        sha256: sha256.to_ascii_lowercase(),
        language: None,
        languages: Vec::new(),
    })
}

pub(crate) fn linuxmint() -> Result<Release, Error> {
    let index = http::text(&format!("{MINT}/"), &[])?;
    let dirs = Regex::new(r#"href="(\d+(?:\.\d+)*)/""#).expect("valid regex");
    let version = dirs
        .captures_iter(&index)
        .map(|c| c[1].to_owned())
        .max_by_key(|v| version_key(v))
        .ok_or_else(|| Error::Format("no Linux Mint release listed".into()))?;
    let file = format!("linuxmint-{version}-cinnamon-64bit.iso");
    let sums = http::text(&format!("{MINT}/{version}/sha256sum.txt"), &[])?;
    release("linuxmint", &version, file.clone(), format!("{MINT}/{version}/{file}"), &sums)
}

pub(crate) fn debian_netinst() -> Result<Release, Error> {
    let sums = http::text(&format!("{DEBIAN_NETINST}/SHA256SUMS"), &[])?;
    let (version, file) = file_in_sums(&sums, r"^debian-(\d+(?:\.\d+)*)-amd64-netinst\.iso$")?;
    release("debian", &version, file.clone(), format!("{DEBIAN_NETINST}/{file}"), &sums)
}

pub(crate) fn debian_live_kde() -> Result<Release, Error> {
    let sums = http::text(&format!("{DEBIAN_LIVE}/SHA256SUMS"), &[])?;
    let (version, file) = file_in_sums(&sums, r"^debian-live-(\d+(?:\.\d+)*)-amd64-kde\.iso$")?;
    release("debian-live-kde", &version, file.clone(), format!("{DEBIAN_LIVE}/{file}"), &sums)
}

pub(crate) fn arch() -> Result<Release, Error> {
    let sums = http::text(&format!("{ARCH}/sha256sums.txt"), &[])?;
    // The dated name, not the "archlinux-x86_64.iso" alias, so cached files are versioned.
    let (version, file) = file_in_sums(&sums, r"^archlinux-(\d{4}\.\d{2}\.\d{2})-x86_64\.iso$")?;
    release("arch", &version, file.clone(), format!("{ARCH}/{file}"), &sums)
}

/// The file in a checksum list whose name matches `pattern`; capture 1 is its version.
fn file_in_sums(sums: &str, pattern: &str) -> Result<(String, String), Error> {
    let re = Regex::new(pattern).expect("valid regex");
    sums.lines()
        .filter_map(|line| line.split_whitespace().nth(1).map(|f| f.trim_start_matches('*')))
        .find_map(|file| re.captures(file).map(|c| (c[1].to_owned(), file.to_owned())))
        .ok_or_else(|| Error::Format(format!("no file matching {pattern} in the checksum list")))
}

fn release(entry: &str, version: &str, file_name: String, url: String, sums: &str) -> Result<Release, Error> {
    let sha256 =
        checksum_for(sums, &file_name).ok_or_else(|| Error::Format(format!("no checksum listed for {file_name}")))?;
    Ok(Release {
        entry: entry.to_owned(),
        version: version.to_owned(),
        size: http::content_length(&url),
        file_name,
        url,
        sha256,
        language: None,
        languages: Vec::new(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn latest_lts_skips_interim_and_odd_years() {
        let index = r#"<a href="24.04/">24.04</a> <a href="25.10/">25.10</a> <a href="26.04/">x</a>
                       <a href="26.04.1/">x</a> <a href="26.10/">x</a> <a href="27.04/">x</a>"#;
        assert_eq!(latest_lts(index).unwrap(), "26.04.1");
        assert!(latest_lts("<html></html>").is_err());
    }

    #[test]
    fn fedora_picks_newest_final_release_of_the_subvariant() {
        let json = r#"[
          {"version":"45 Beta","arch":"x86_64","variant":"KDE","subvariant":"KDE","link":"https://x/45b.iso","sha256":"aa","size":"1"},
          {"version":"44","arch":"x86_64","variant":"KDE","subvariant":"KDE","link":"https://x/Fedora-KDE-44.iso",
           "sha256":"C8295961D4000000000000000000000000000000000000000000000000000000","size":"3368683520"},
          {"version":"44","arch":"aarch64","variant":"KDE","subvariant":"KDE","link":"https://x/arm.iso","sha256":"bb","size":"1"},
          {"version":"43","arch":"x86_64","variant":"KDE","subvariant":"KDE","link":"https://x/43.iso","sha256":"cc","size":"1"},
          {"version":"44","arch":"x86_64","variant":"Workstation","subvariant":"Workstation","link":"https://x/ws.iso","sha256":"dd","size":"1"}
        ]"#;
        let r = pick_fedora("fedora-kde", "KDE", serde_json::from_str(json).unwrap()).unwrap();
        assert_eq!((r.version.as_str(), r.file_name.as_str(), r.size), ("44", "Fedora-KDE-44.iso", Some(3368683520)));
        assert!(r.sha256.starts_with("c8295961d4"), "lower-cased");
    }

    #[test]
    fn dated_arch_file_not_the_alias() {
        let sums = "684d  archlinux-x86_64.iso\n684d  archlinux-2026.10.01-x86_64.iso\n0c9c  archlinux-bootstrap-2026.10.01-x86_64.tar.zst\n";
        let (version, file) = file_in_sums(sums, r"^archlinux-(\d{4}\.\d{2}\.\d{2})-x86_64\.iso$").unwrap();
        assert_eq!((version.as_str(), file.as_str()), ("2026.10.01", "archlinux-2026.10.01-x86_64.iso"));
    }
}
