//! Updates: which release is newer, what the update feed says, and the
//! state the resident shares with terminals. Pure; the network and the
//! installer are in `updater` and `sys`.
//!
//! The feed is two text files on the `update-feed` GitHub release, written
//! by release.yml: `stable.txt` and `nightly.txt`, each
//!
//! ```text
//! version 0.2.0
//! notes https://github.com/UlloaSP/aeon/releases/tag/v0.2.0
//! x64 https://…/aeon-0.2.0-windows-x64.msi <sha256>
//! arm64 https://…/aeon-0.2.0-windows-arm64.msi <sha256>
//! ```

use std::cmp::Ordering;
use std::fmt;

pub const FEED: &str = "https://github.com/UlloaSP/aeon/releases/download/update-feed";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Stable,
    Nightly,
}

/// A semver version: `1.2.3` or `1.2.3-nightly.20261124.4`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Version {
    core: [u64; 3],
    pre: Vec<String>,
}

impl Version {
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim().trim_start_matches('v');
        let (core, pre) = match text.split_once('-') {
            Some((core, pre)) => (core, pre.split('.').map(str::to_owned).collect()),
            None => (text, Vec::new()),
        };
        let mut parts = core.split('.').map(|p| p.parse::<u64>());
        let core = [
            parts.next()?.ok()?,
            parts.next()?.ok()?,
            parts.next()?.ok()?,
        ];
        parts.next().is_none().then_some(Self { core, pre })
    }
}

impl Ord for Version {
    /// Semver precedence: a prerelease sorts before its release, and
    /// numeric identifiers compare as numbers (`.10` after `.9`).
    fn cmp(&self, other: &Self) -> Ordering {
        self.core
            .cmp(&other.core)
            .then_with(|| match (self.pre.is_empty(), other.pre.is_empty()) {
                (true, true) => Ordering::Equal,
                (true, false) => Ordering::Greater,
                (false, true) => Ordering::Less,
                (false, false) => {
                    for (a, b) in self.pre.iter().zip(&other.pre) {
                        let order = match (a.parse::<u64>(), b.parse::<u64>()) {
                            (Ok(x), Ok(y)) => x.cmp(&y),
                            (Ok(_), Err(_)) => Ordering::Less,
                            (Err(_), Ok(_)) => Ordering::Greater,
                            (Err(_), Err(_)) => a.cmp(b),
                        };
                        if order != Ordering::Equal {
                            return order;
                        }
                    }
                    self.pre.len().cmp(&other.pre.len())
                }
            })
    }
}

impl PartialOrd for Version {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let [a, b, c] = self.core;
        write!(f, "{a}.{b}.{c}")?;
        if !self.pre.is_empty() {
            write!(f, "-{}", self.pre.join("."))?;
        }
        Ok(())
    }
}

/// One release as the feed describes it, for this machine's architecture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub notes: String,
    pub msi_url: String,
    /// Lowercase hex SHA-256 of the MSI.
    pub sha256: String,
}

/// Reads a feed file for `arch` (`x64` or `arm64`).
pub fn parse_feed(text: &str, arch: &str) -> Option<Release> {
    let mut version = None;
    let mut notes = String::new();
    let mut msi = None;
    for line in text.lines() {
        let mut words = line.split_whitespace();
        match (words.next(), words.next(), words.next()) {
            (Some("version"), Some(v), _) => version = Version::parse(v).map(|_| v.to_owned()),
            (Some("notes"), Some(url), _) => notes = url.to_owned(),
            (Some(a), Some(url), Some(sha)) if a == arch && is_sha256(sha) => {
                msi = Some((url.to_owned(), sha.to_lowercase()))
            }
            _ => {}
        }
    }
    let (msi_url, sha256) = msi?;
    Some(Release {
        version: version?,
        notes,
        msi_url,
        sha256,
    })
}

fn is_sha256(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|b| b.is_ascii_hexdigit())
}

/// The release to offer: the newest one the channel allows, if it is newer
/// than `current`. Nightly users get stable releases too when they are newer.
pub fn pick(
    channel: Channel,
    current: &str,
    stable: Option<Release>,
    nightly: Option<Release>,
) -> Option<Release> {
    let current = Version::parse(current)?;
    let candidates = match channel {
        Channel::Stable => vec![stable],
        Channel::Nightly => vec![stable, nightly],
    };
    candidates
        .into_iter()
        .flatten()
        .filter_map(|r| Version::parse(&r.version).map(|v| (v, r)))
        .filter(|(v, _)| *v > current)
        .max_by(|(a, _), (b, _)| a.cmp(b))
        .map(|(_, r)| r)
}

/// What the resident is doing about updates; written to a small file that
/// terminals read for their sidebar and settings.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Status {
    #[default]
    Idle,
    Checking,
    UpToDate,
    Available(Release),
    Downloading {
        release: Release,
        percent: u8,
    },
    /// Downloaded and verified; the MSI is at `path`.
    Ready {
        release: Release,
        path: String,
    },
    Error(String),
}

impl Status {
    /// `key value` lines, one field each.
    pub fn to_text(&self) -> String {
        let release = |r: &Release| {
            format!(
                "version {}\nnotes {}\nmsi {}\nsha256 {}\n",
                r.version, r.notes, r.msi_url, r.sha256
            )
        };
        match self {
            Status::Idle => "state idle\n".to_owned(),
            Status::Checking => "state checking\n".to_owned(),
            Status::UpToDate => "state up-to-date\n".to_owned(),
            Status::Available(r) => format!("state available\n{}", release(r)),
            Status::Downloading {
                release: r,
                percent,
            } => {
                format!("state downloading\npercent {percent}\n{}", release(r))
            }
            Status::Ready { release: r, path } => {
                format!("state ready\npath {path}\n{}", release(r))
            }
            Status::Error(message) => format!("state error\nmessage {message}\n"),
        }
    }

    pub fn parse(text: &str) -> Status {
        let field = |key: &str| {
            text.lines()
                .find_map(|l| l.strip_prefix(key)?.strip_prefix(' '))
                .unwrap_or("")
                .to_owned()
        };
        let release = || Release {
            version: field("version"),
            notes: field("notes"),
            msi_url: field("msi"),
            sha256: field("sha256"),
        };
        match field("state").as_str() {
            "checking" => Status::Checking,
            "up-to-date" => Status::UpToDate,
            "available" => Status::Available(release()),
            "downloading" => Status::Downloading {
                release: release(),
                percent: field("percent").parse().unwrap_or(0),
            },
            "ready" => Status::Ready {
                release: release(),
                path: field("path"),
            },
            "error" => Status::Error(field("message")),
            _ => Status::Idle,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(text: &str) -> Version {
        Version::parse(text).unwrap()
    }

    #[test]
    fn versions_order_like_semver() {
        assert!(v("0.2.0") > v("0.1.9"));
        assert!(v("0.2.0") > v("0.2.0-nightly.20261124.9"));
        assert!(v("0.2.1-nightly.20261124.10") > v("0.2.1-nightly.20261124.9"));
        assert!(v("0.2.1-nightly.20261125.1") > v("0.2.1-nightly.20261124.9"));
        assert_eq!(v("v1.2.3"), v("1.2.3"));
        assert_eq!(v("1.2.3-nightly.1").to_string(), "1.2.3-nightly.1");
        for bad in ["1.2", "1.2.3.4", "x.y.z", ""] {
            assert!(Version::parse(bad).is_none(), "{bad:?}");
        }
    }

    const SHA: &str = "a3f1c2d4e5b60718293a4b5c6d7e8f90a1b2c3d4e5f60718293a4b5c6d7e8f90";

    fn feed(version: &str) -> String {
        format!(
            "version {version}\nnotes https://example/notes\nx64 https://example/x64.msi {SHA}\narm64 https://example/arm64.msi {SHA}\n"
        )
    }

    #[test]
    fn feeds_parse_per_architecture() {
        let release = parse_feed(&feed("0.2.0"), "arm64").unwrap();
        assert_eq!(release.msi_url, "https://example/arm64.msi");
        assert_eq!(release.sha256, SHA);
        assert_eq!(release.version, "0.2.0");
        assert!(parse_feed(&feed("0.2.0"), "x86").is_none());
        assert!(parse_feed("version 0.2.0\nx64 https://e/x.msi not-a-hash\n", "x64").is_none());
    }

    #[test]
    fn channels_pick_the_newest_allowed_release() {
        let stable = parse_feed(&feed("0.2.0"), "x64");
        let nightly = parse_feed(&feed("0.2.1-nightly.20261124.3"), "x64");
        let pick_version = |channel, current| {
            pick(channel, current, stable.clone(), nightly.clone()).map(|r| r.version)
        };
        assert_eq!(
            pick_version(Channel::Stable, "0.1.0").as_deref(),
            Some("0.2.0")
        );
        assert_eq!(pick_version(Channel::Stable, "0.2.0"), None);
        assert_eq!(
            pick_version(Channel::Nightly, "0.2.0").as_deref(),
            Some("0.2.1-nightly.20261124.3")
        );
        assert_eq!(
            pick_version(Channel::Nightly, "0.2.1-nightly.20261124.3"),
            None
        );
        // A local build that is not semver is never offered anything.
        assert_eq!(pick_version(Channel::Stable, "dev"), None);
    }

    #[test]
    fn status_round_trips_through_text() {
        let release = parse_feed(&feed("0.2.0"), "x64").unwrap();
        for status in [
            Status::Idle,
            Status::Checking,
            Status::UpToDate,
            Status::Available(release.clone()),
            Status::Downloading {
                release: release.clone(),
                percent: 42,
            },
            Status::Ready {
                release,
                path: r"C:\Users\me\AppData\Local\aeon\updates\aeon.msi".into(),
            },
            Status::Error("could not reach GitHub".into()),
        ] {
            assert_eq!(Status::parse(&status.to_text()), status);
        }
    }
}
