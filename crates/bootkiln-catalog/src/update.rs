//! "Is there a newer bootkiln?" via GitHub's releases API. Notify only: nothing is
//! downloaded or installed.

use serde::{Deserialize, Serialize};

use crate::{Error, http, version_key};

const LATEST_RELEASE: &str = "https://api.github.com/repos/yunodu/bootkiln/releases/latest";

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Update {
    pub version: String,
    /// The release page.
    pub url: String,
}

#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    html_url: String,
}

/// The latest published release, if it is newer than `current` (e.g. `"0.1.0"`).
pub fn newer_release(current: &str) -> Result<Option<Update>, Error> {
    let body = http::text(LATEST_RELEASE, &[("Accept", "application/vnd.github+json")])?;
    let release: GitHubRelease =
        serde_json::from_str(&body).map_err(|e| Error::Format(format!("GitHub releases API: {e}")))?;
    Ok(newer(current, release))
}

fn newer(current: &str, release: GitHubRelease) -> Option<Update> {
    let version = release.tag_name.trim_start_matches('v').to_owned();
    let (latest, current) = (version_key(&version)?, version_key(current)?);
    (latest > current).then_some(Update { version, url: release.html_url })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(tag: &str) -> GitHubRelease {
        GitHubRelease { tag_name: tag.into(), html_url: format!("https://github.com/yunodu/bootkiln/releases/tag/{tag}") }
    }

    #[test]
    fn only_strictly_newer_versions_are_offered() {
        assert_eq!(newer("0.1.0", release("v0.2.0")).map(|u| u.version), Some("0.2.0".into()));
        assert_eq!(newer("0.1.0", release("v0.1.10")).map(|u| u.version), Some("0.1.10".into()));
        assert_eq!(newer("0.2.0", release("v0.2.0")), None);
        assert_eq!(newer("0.3.0", release("v0.2.9")), None);
        assert_eq!(newer("0.1.0", release("nightly")), None);
    }
}
