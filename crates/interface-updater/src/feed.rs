use std::{fmt, path::Path};

use interface_contracts::updater::UpdateSource;
use thiserror::Error;
use url::Url;

const MAX_FEED_URL_BYTES: usize = 2_048;
const MAX_FEED_FILE_NAME_BYTES: usize = 220;

#[derive(Clone, Eq, PartialEq)]
pub struct NormalizedFeed {
    url: Url,
    source: UpdateSource,
}

impl NormalizedFeed {
    /// Validates and canonicalizes a local or remote update feed.
    ///
    /// # Errors
    ///
    /// Returns an error for malformed, credential-bearing, query-bearing,
    /// non-loopback local, non-HTTPS remote, or `none` feeds.
    pub fn new(raw: &str, source: UpdateSource) -> Result<Self, FeedError> {
        if raw.is_empty() || raw.len() > MAX_FEED_URL_BYTES {
            return Err(FeedError::MissingOrTooLong);
        }
        let url = Url::parse(raw).map_err(|_| FeedError::Invalid)?;
        if !url.username().is_empty() || url.password().is_some() {
            return Err(FeedError::Credentials);
        }
        if url.query().is_some() || url.fragment().is_some() {
            return Err(FeedError::QueryOrFragment);
        }
        match source {
            UpdateSource::Local => {
                let loopback = url.host_str().is_some_and(|host| {
                    host.eq_ignore_ascii_case("localhost")
                        || host == "127.0.0.1"
                        || matches!(host, "::1" | "[::1]")
                });
                if url.scheme() != "http" || !loopback {
                    return Err(FeedError::LocalNotLoopbackHttp);
                }
            }
            UpdateSource::Remote => {
                if url.scheme() != "https" {
                    return Err(FeedError::RemoteNotHttps);
                }
            }
            UpdateSource::None => return Err(FeedError::InvalidSource),
        }
        Ok(Self { url, source })
    }

    /// The public channel's feed, already admitted by `ReleaseChannel`.
    pub(crate) const fn channel(url: Url) -> Self {
        Self {
            url,
            source: UpdateSource::Remote,
        }
    }

    #[must_use]
    pub const fn source(&self) -> UpdateSource {
        self.source
    }

    /// Returns the canonical URL only to the native updater boundary. It must
    /// never be copied into renderer-visible state or error text.
    #[must_use]
    pub fn as_url(&self) -> &Url {
        &self.url
    }
}

impl fmt::Debug for NormalizedFeed {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("NormalizedFeed")
            .field("source", &self.source)
            .field("origin", &self.url.origin().ascii_serialization())
            .finish_non_exhaustive()
    }
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum FeedError {
    #[error("update feed URL is missing or too long")]
    MissingOrTooLong,
    #[error("update feed URL is invalid")]
    Invalid,
    #[error("update feed URLs cannot contain credentials")]
    Credentials,
    #[error("update feed URLs cannot contain query parameters or fragments")]
    QueryOrFragment,
    #[error("local update feeds must use loopback HTTP")]
    LocalNotLoopbackHttp,
    #[error("remote update feeds must use HTTPS")]
    RemoteNotHttps,
    #[error("update feed source is invalid")]
    InvalidSource,
}

/// Narrows a loopback request URL to one flat Electron migration artifact.
#[must_use]
pub fn update_feed_file_name(request_url: Option<&str>) -> Option<String> {
    let request_url = request_url?;
    let base = Url::parse("http://127.0.0.1").ok()?;
    let url = base.join(request_url).ok()?;
    let pathname = percent_decode(url.path().as_bytes())?;
    let name = pathname.strip_prefix('/')?;
    if name.contains('/') {
        return None;
    }
    if name == "latest-mac.yml" {
        return Some(name.to_owned());
    }
    if name.is_empty()
        || name.len() > MAX_FEED_FILE_NAME_BYTES
        || name.contains('\\')
        || !name
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphanumeric)
        || !name.bytes().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(byte, b'.' | b'_' | b'+' | b' ' | b'(' | b')' | b'-')
        })
    {
        return None;
    }
    matches!(
        Path::new(name)
            .extension()
            .and_then(|extension| extension.to_str()),
        Some("zip" | "dmg" | "blockmap")
    )
    .then(|| name.to_owned())
}

#[must_use]
pub fn content_type(name: &str) -> &'static str {
    match Path::new(name).extension().and_then(|value| value.to_str()) {
        Some("yml") => "application/yaml; charset=utf-8",
        Some("zip") => "application/zip",
        Some("dmg") => "application/x-apple-diskimage",
        _ => "application/octet-stream",
    }
}

fn percent_decode(bytes: &[u8]) -> Option<String> {
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' {
            let high = *bytes.get(index + 1)?;
            let low = *bytes.get(index + 2)?;
            decoded.push(hex(high)?.checked_mul(16)?.checked_add(hex(low)?)?);
            index += 3;
        } else {
            decoded.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(decoded).ok()
}

const fn hex(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_allowlist_is_flat_bounded_and_percent_aware() {
        for accepted in [
            "/latest-mac.yml",
            "/Interface-0.2.0-arm64.zip",
            "/Interface%200.2.0.dmg",
            "/Interface.zip.blockmap",
        ] {
            assert!(
                update_feed_file_name(Some(accepted)).is_some(),
                "{accepted}"
            );
        }
        for refused in [
            "/",
            "/nested/update.zip",
            "/secret.txt",
            "/%2e%2e%2fsecret.zip",
            "/bad%ZZ.zip",
            "/bad\\name.zip",
        ] {
            assert_eq!(update_feed_file_name(Some(refused)), None, "{refused}");
        }
    }

    #[test]
    fn feed_policy_accepts_only_loopback_http_or_remote_https() {
        for local in [
            "http://localhost:8080/releases",
            "http://127.0.0.1:43119/",
            "http://[::1]:8080/releases",
        ] {
            assert!(NormalizedFeed::new(local, UpdateSource::Local).is_ok());
        }
        assert!(
            NormalizedFeed::new("https://updates.example.test/path", UpdateSource::Remote).is_ok()
        );
        assert_eq!(
            NormalizedFeed::new("http://updates.example.test", UpdateSource::Remote),
            Err(FeedError::RemoteNotHttps)
        );
        assert_eq!(
            NormalizedFeed::new("http://192.168.1.2", UpdateSource::Local),
            Err(FeedError::LocalNotLoopbackHttp)
        );
        assert_eq!(
            NormalizedFeed::new("https://user:secret@example.test", UpdateSource::Remote),
            Err(FeedError::Credentials)
        );
        assert_eq!(
            NormalizedFeed::new("https://example.test/?token=secret", UpdateSource::Remote),
            Err(FeedError::QueryOrFragment)
        );
    }

    #[test]
    fn debug_output_never_contains_private_feed_paths() {
        let feed = NormalizedFeed::new(
            "https://updates.example.test/private/account/channel/",
            UpdateSource::Remote,
        )
        .expect("feed");
        let debug = format!("{feed:?}");
        assert!(debug.contains("updates.example.test"));
        assert!(!debug.contains("private"));
        assert!(!debug.contains("account"));
    }
}
