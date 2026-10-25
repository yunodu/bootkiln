//! Shared HTTP agent: HTTPS (rustls), redirects, and timeouts that do not cut long downloads.

use std::sync::LazyLock;
use std::time::Duration;

use ureq::Agent;

use crate::Error;

/// Some servers (Microsoft's) answer differently to unknown clients; this mirrors a browser.
pub(crate) const BROWSER_USER_AGENT: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/140.0 Safari/537.36";
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
/// Time to the response headers; bodies may take hours.
const RESPONSE_TIMEOUT: Duration = Duration::from_secs(60);

static AGENT: LazyLock<Agent> = LazyLock::new(|| {
    Agent::config_builder()
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_recv_response(Some(RESPONSE_TIMEOUT))
        .user_agent(BROWSER_USER_AGENT)
        .build()
        .into()
});

pub(crate) fn agent() -> &'static Agent {
    &AGENT
}

/// GET `url` as text. Pages and listings are small; this refuses anything over 16 MiB.
pub(crate) fn text(url: &str, headers: &[(&str, &str)]) -> Result<String, Error> {
    let mut request = agent().get(url);
    for (name, value) in headers {
        request = request.header(*name, *value);
    }
    let mut response = request.call().map_err(|e| Error::Http(format!("{url}: {e}")))?;
    response
        .body_mut()
        .with_config()
        .limit(16 * 1024 * 1024)
        .read_to_string()
        .map_err(|e| Error::Http(format!("{url}: {e}")))
}

/// Size of the file at `url` from a HEAD request, if the server reports it.
pub(crate) fn content_length(url: &str) -> Option<u64> {
    let response = agent().head(url).call().ok()?;
    response.headers().get("content-length")?.to_str().ok()?.parse().ok()
}
