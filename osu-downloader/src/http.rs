//! HTTP client creation and configuration (internal).

use crate::Result;
use std::time::Duration;

pub(crate) const DEFAULT_DOWNLOAD_CONNECT_TIMEOUT: Duration = Duration::from_secs(30);

#[cfg(any(feature = "collection", feature = "size-fetch", feature = "search"))]
pub(crate) const DEFAULT_API_TIMEOUT: Duration = Duration::from_secs(15);

pub(crate) fn create_download_client(user_agent: Option<String>) -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(DEFAULT_DOWNLOAD_CONNECT_TIMEOUT)
        // Redirects are followed by hand in `download::try_mirror_once` so the
        // mirror's budget headers on its own 30x response stay visible.
        .redirect(reqwest::redirect::Policy::none())
        .pool_max_idle_per_host(10);

    if let Some(ua) = user_agent {
        builder = builder.user_agent(ua);
    }

    builder.build().map_err(Into::into)
}

#[cfg(any(feature = "collection", feature = "size-fetch", feature = "search"))]
pub(crate) fn create_api_client(user_agent: Option<&str>) -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .timeout(DEFAULT_API_TIMEOUT)
        .pool_max_idle_per_host(20);

    if let Some(ua) = user_agent {
        builder = builder.user_agent(ua);
    }

    builder.build().map_err(Into::into)
}

#[cfg(all(
    test,
    any(feature = "collection", feature = "size-fetch", feature = "search")
))]
#[path = "../tests/http.rs"]
mod tests;
