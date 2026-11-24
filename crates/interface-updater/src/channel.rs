//! The public update channel compiled into this build.
//!
//! `release-channel.json` beside this crate's manifest names where the signed
//! release manifest is published and which keys may sign it. Both are part of
//! the binary: nothing at run time, no environment variable and no file in
//! the data folder, can point a packaged app at another feed or key. With no
//! feed or no key listed the channel is off and the app updates only from a
//! feed staged on this Mac.

use serde::Deserialize;
use thiserror::Error;
use url::Url;

use interface_contracts::updater::UpdateSource;

use crate::NormalizedFeed;
use crate::signed_manifest::PublicKey;

const COMPILED_CHANNEL: &str = include_str!("../release-channel.json");
const CHANNEL_SCHEMA: u32 = 1;
const MAX_FEEDS: usize = 4;
const MAX_KEYS: usize = 4;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReleaseChannel {
    product: String,
    channel: String,
    feeds: Vec<Url>,
    keys: Vec<PublicKey>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ChannelFile {
    schema: u32,
    product: String,
    channel: String,
    feeds: Vec<String>,
    keys: Vec<String>,
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum ChannelError {
    #[error("the release channel file is malformed")]
    Malformed,
    #[error(
        "a release feed must be an HTTPS address of the manifest file, without credentials or a query"
    )]
    InvalidFeed,
    #[error("a release key must be a base64 Ed25519 public key, listed once")]
    InvalidKey,
    #[error("a release channel lists at most four feeds and four keys")]
    TooMany,
}

impl ReleaseChannel {
    /// The channel this build was compiled with, if one is switched on.
    #[must_use]
    pub fn compiled() -> Option<Self> {
        Self::parse(COMPILED_CHANNEL).ok().flatten()
    }

    /// Reads a channel file. `Ok(None)` is a well-formed file with the
    /// channel off: no feed, or no key.
    ///
    /// # Errors
    ///
    /// Refuses a malformed file, a feed that is not plain HTTPS and a key
    /// that is not an Ed25519 public key.
    pub fn parse(text: &str) -> Result<Option<Self>, ChannelError> {
        let file: ChannelFile = serde_json::from_str(text).map_err(|_| ChannelError::Malformed)?;
        if file.schema != CHANNEL_SCHEMA || file.product.is_empty() || file.channel.is_empty() {
            return Err(ChannelError::Malformed);
        }
        if file.feeds.len() > MAX_FEEDS || file.keys.len() > MAX_KEYS {
            return Err(ChannelError::TooMany);
        }
        let feeds = file
            .feeds
            .iter()
            .map(|feed| manifest_url(feed, UpdateSource::Remote))
            .collect::<Result<Vec<_>, _>>()?;
        let mut keys = Vec::with_capacity(file.keys.len());
        for text in &file.keys {
            let key = PublicKey::from_base64(text).ok_or(ChannelError::InvalidKey)?;
            if keys.contains(&key) {
                return Err(ChannelError::InvalidKey);
            }
            keys.push(key);
        }
        if feeds.is_empty() || keys.is_empty() {
            return Ok(None);
        }
        Ok(Some(Self {
            product: file.product,
            channel: file.channel,
            feeds,
            keys,
        }))
    }

    /// A channel served from this Mac over loopback HTTP, for tests and the
    /// publisher's rehearsal. Never built from configuration.
    ///
    /// # Errors
    ///
    /// Refuses anything but loopback HTTP feeds.
    pub fn loopback(
        product: &str,
        channel: &str,
        feeds: &[&str],
        keys: &[PublicKey],
    ) -> Result<Self, ChannelError> {
        let feeds = feeds
            .iter()
            .map(|feed| manifest_url(feed, UpdateSource::Local))
            .collect::<Result<Vec<_>, _>>()?;
        if feeds.is_empty() || keys.is_empty() {
            return Err(ChannelError::Malformed);
        }
        Ok(Self {
            product: product.to_owned(),
            channel: channel.to_owned(),
            feeds,
            keys: keys.to_vec(),
        })
    }

    #[must_use]
    pub fn product(&self) -> &str {
        &self.product
    }

    #[must_use]
    pub fn channel(&self) -> &str {
        &self.channel
    }

    /// Where the signed manifest is published, in the order to try.
    #[must_use]
    pub fn feeds(&self) -> &[Url] {
        &self.feeds
    }

    #[must_use]
    pub fn keys(&self) -> &[PublicKey] {
        &self.keys
    }
}

fn manifest_url(feed: &str, source: UpdateSource) -> Result<Url, ChannelError> {
    let url = NormalizedFeed::new(feed, source)
        .map_err(|_| ChannelError::InvalidFeed)?
        .as_url()
        .clone();
    // The archive is fetched from beside the manifest, so the address must
    // name a file, not a folder.
    if url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .is_none_or(str::is_empty)
    {
        return Err(ChannelError::InvalidFeed);
    }
    Ok(url)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::signed_manifest::generate_signing_key;

    fn file(feeds: &[&str], keys: &[String]) -> String {
        serde_json::json!({
            "schema": 1, "product": "com.example.interface", "channel": "stable",
            "feeds": feeds, "keys": keys,
        })
        .to_string()
    }

    #[test]
    fn the_channel_file_in_the_repository_is_well_formed() {
        // Off (no feed or key yet) or on; never a file this build cannot read.
        assert!(ReleaseChannel::parse(COMPILED_CHANNEL).is_ok());
    }

    #[test]
    fn a_channel_is_off_until_it_has_both_a_feed_and_a_key() {
        let (_, key) = generate_signing_key().expect("key");
        let feed = "https://updates.example.test/interface-update.json";
        assert_eq!(ReleaseChannel::parse(&file(&[], &[])), Ok(None));
        assert_eq!(ReleaseChannel::parse(&file(&[feed], &[])), Ok(None));
        assert_eq!(
            ReleaseChannel::parse(&file(&[], &[key.to_base64()])),
            Ok(None)
        );
        let channel = ReleaseChannel::parse(&file(&[feed], &[key.to_base64()]))
            .expect("parsed")
            .expect("on");
        assert_eq!(channel.feeds()[0].as_str(), feed);
        assert_eq!(channel.keys(), [key]);
        assert_eq!(channel.product(), "com.example.interface");
    }

    #[test]
    fn only_plain_https_manifest_addresses_are_feeds() {
        let (_, key) = generate_signing_key().expect("key");
        for feed in [
            "http://updates.example.test/interface-update.json",
            "http://127.0.0.1:8080/interface-update.json",
            "https://user:secret@updates.example.test/interface-update.json",
            "https://updates.example.test/interface-update.json?token=1",
            "https://updates.example.test/releases/",
            "file:///tmp/interface-update.json",
        ] {
            assert_eq!(
                ReleaseChannel::parse(&file(&[feed], &[key.to_base64()])),
                Err(ChannelError::InvalidFeed),
                "{feed}"
            );
        }
    }

    #[test]
    fn a_key_that_is_not_a_key_or_is_listed_twice_is_refused() {
        let (_, key) = generate_signing_key().expect("key");
        let feed = "https://updates.example.test/interface-update.json";
        assert_eq!(
            ReleaseChannel::parse(&file(&[feed], &["bm90IGEga2V5".to_owned()])),
            Err(ChannelError::InvalidKey)
        );
        assert_eq!(
            ReleaseChannel::parse(&file(&[feed], &[key.to_base64(), key.to_base64()])),
            Err(ChannelError::InvalidKey)
        );
    }

    #[test]
    fn a_loopback_channel_takes_only_loopback_feeds() {
        let (_, key) = generate_signing_key().expect("key");
        assert!(
            ReleaseChannel::loopback(
                "p",
                "stable",
                &["http://127.0.0.1:9/interface-update.json"],
                &[key]
            )
            .is_ok()
        );
        assert_eq!(
            ReleaseChannel::loopback(
                "p",
                "stable",
                &["https://updates.example.test/interface-update.json"],
                &[key]
            ),
            Err(ChannelError::InvalidFeed)
        );
    }
}
