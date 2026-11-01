//! Adapter for the official Bitwarden CLI.
//!
//! Countersign never implements Bitwarden's encryption or talks to its
//! servers. Every vault operation goes through the official `bw` executable:
//! short commands such as `bw status` run once, and vault work goes to a
//! long-running `bw serve` child that listens only on a private socket pair.

mod cli;
mod error;
mod serve;
mod session;
mod version;

pub use cli::{AuthStatus, Cli, CliStatus};
pub use error::{BwError, Result};
pub use serve::{GenerateOptions, ListOptions, Serve, ServeClient};
pub use session::SessionKey;
pub use version::{CliVersion, MINIMUM_VERSION, PINNED_ARCHIVES, PINNED_VERSION, PinnedArchive};

/// Checks that `id` has the shape of a Bitwarden object id before it is
/// placed in a request path.
pub fn is_object_id(id: &str) -> bool {
    id.len() == 36
        && id.char_indices().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => c == '-',
            _ => c.is_ascii_hexdigit(),
        })
}

#[cfg(test)]
mod tests {
    use super::is_object_id;

    #[test]
    fn object_ids_are_strict() {
        assert!(is_object_id("0b6a1c4e-1f2d-4c3b-9a8e-7d6c5b4a3f21"));
        assert!(!is_object_id("../object/item"));
        assert!(!is_object_id("0b6a1c4e-1f2d-4c3b-9a8e-7d6c5b4a3f2"));
        assert!(!is_object_id("0b6a1c4e_1f2d-4c3b-9a8e-7d6c5b4a3f21"));
    }
}
