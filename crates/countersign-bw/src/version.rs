use std::fmt;

/// A stable, numeric release of the official CLI, such as `2026.10.1`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CliVersion {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

/// The oldest release whose `bw serve` contract Countersign has verified.
pub const MINIMUM_VERSION: CliVersion = CliVersion::new(2026, 10, 0);

/// The release Countersign installs when it manages the CLI itself.
pub const PINNED_VERSION: CliVersion = CliVersion::new(2026, 10, 1);

/// One official release archive of [`PINNED_VERSION`], with the SHA-256 that
/// GitHub publishes for it. Update both whenever the pin changes.
#[derive(Debug, Clone, Copy)]
pub struct PinnedArchive {
    /// `std::env::consts::ARCH` of the machines it runs on.
    pub arch: &'static str,
    pub file: &'static str,
    pub sha256: &'static str,
}

pub const PINNED_ARCHIVES: &[PinnedArchive] = &[
    PinnedArchive {
        arch: "x86_64",
        file: "bw-linux-2026.10.1.zip",
        sha256: "eb09c3715291c21cf744819b873a255c1247b1b8b45bae138621a644ec7ad8f5",
    },
    PinnedArchive {
        arch: "aarch64",
        file: "bw-linux-arm64-2026.10.1.zip",
        sha256: "79ecb3293fb259660961c949d3e32fdab8682b2bcff0f6764ce38a411ca1ac5d",
    },
];

impl PinnedArchive {
    /// The archive for this machine, if Bitwarden publishes one.
    pub fn for_this_machine() -> Option<&'static PinnedArchive> {
        PINNED_ARCHIVES.iter().find(|a| a.arch == std::env::consts::ARCH)
    }

    pub fn url(&self) -> String {
        format!("https://github.com/bitwarden/clients/releases/download/cli-v{PINNED_VERSION}/{}", self.file)
    }
}

impl CliVersion {
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        CliVersion { major, minor, patch }
    }

    /// Parses `bw --version` output. Prereleases, development builds and
    /// anything other than three numeric components are refused.
    pub fn parse(output: &str) -> Option<Self> {
        let text = output.trim();
        let mut parts = text.split('.');
        let mut next = || -> Option<u32> {
            let part = parts.next()?;
            if part.is_empty() || part.len() > 9 || !part.bytes().all(|b| b.is_ascii_digit()) {
                return None;
            }
            part.parse().ok()
        };
        let version = CliVersion::new(next()?, next()?, next()?);
        parts.next().is_none().then_some(version)
    }

    pub fn is_supported(self) -> bool {
        self >= MINIMUM_VERSION
    }
}

impl fmt::Display for CliVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stable_versions_only() {
        assert_eq!(CliVersion::parse("2026.10.1\n"), Some(CliVersion::new(2026, 10, 1)));
        assert_eq!(CliVersion::parse("2026.10.0-beta.1"), None);
        assert_eq!(CliVersion::parse("2026.10"), None);
        assert_eq!(CliVersion::parse("2026.10.1.2"), None);
        assert_eq!(CliVersion::parse("v2026.10.1"), None);
        assert_eq!(CliVersion::parse(""), None);
    }

    #[test]
    fn compares_numerically() {
        assert!(CliVersion::new(2026, 10, 0) > CliVersion::new(2026, 9, 1));
        assert!(CliVersion::new(2026, 10, 0).is_supported());
        assert!(!CliVersion::new(2026, 9, 1).is_supported());
        assert!(PINNED_VERSION.is_supported());
    }

    #[test]
    fn pinned_archives_match_the_pin() {
        for archive in PINNED_ARCHIVES {
            assert!(archive.file.contains(&PINNED_VERSION.to_string()), "{}", archive.file);
            assert_eq!(archive.sha256.len(), 64);
            assert!(
                archive.url().starts_with("https://github.com/bitwarden/clients/releases/download/cli-v")
            );
        }
    }
}
