//! Changes the CLI's lists do not show yet.
//!
//! `bw serve` 2026.10.1 often answers a list from before the latest change:
//! an edited item keeps its old name, a renamed folder its old name, a new
//! item or folder is missing. The list stays behind until a later change or
//! a sync, while reading the one object by id answers current. So after each
//! change the daemon reads the object back, keeps that version here, and lays
//! it over every list until the list catches up.

use std::collections::HashMap;

use countersign_ipc::{Folder, ItemSummary, VaultSnapshot};

#[derive(Debug, Default)]
pub(crate) struct Unlisted {
    /// Items as read back after a change; `None` once deleted for good.
    items: HashMap<String, Option<ItemSummary>>,
    /// Folders as a change returned them, `None` once deleted, with the
    /// number of the change.
    folders: HashMap<String, (Option<Folder>, u64)>,
    /// Changes recorded so far.
    changes: u64,
}

/// An object as it is after a change, read back from the CLI.
#[expect(clippy::large_enum_variant, reason = "one short-lived value per change")]
pub(crate) enum Known {
    /// An item, or `None` once deleted for good.
    Item(String, Option<ItemSummary>),
    /// A folder, or `None` once deleted.
    Folder(String, Option<Folder>),
}

impl Unlisted {
    pub(crate) fn record(&mut self, known: Known) {
        self.changes += 1;
        match known {
            Known::Item(id, item) => drop(self.items.insert(id, item)),
            Known::Folder(id, folder) => drop(self.folders.insert(id, (folder, self.changes))),
        }
    }

    /// Marks the start of a sync, for `forget_folders_before`.
    pub(crate) fn mark(&self) -> u64 {
        self.changes
    }

    /// Forgets the folder changes recorded before a sync started. A folder
    /// has no revision, so a name changed on another device could not be
    /// told apart from a list that is behind. A change made during the sync
    /// stays. Items keep their entries: their revisions settle it.
    pub(crate) fn forget_folders_before(&mut self, mark: u64) {
        self.folders.retain(|_, (_, change)| *change > mark);
    }

    /// Corrects a snapshot built from the CLI's lists, and forgets each
    /// change the lists now show.
    pub(crate) fn apply(&mut self, snapshot: &mut VaultSnapshot) {
        self.items.retain(|id, known| {
            let listed = snapshot.items.iter().chain(&snapshot.trash).find(|i| &i.id == id);
            let caught_up = match (known, listed) {
                (None, listed) => listed.is_none(),
                // A newer revision is a change from elsewhere, which wins.
                // Favorites and folders change without a new revision, so
                // the same revision is not enough.
                (Some(known), Some(listed)) => listed == known || listed.revision > known.revision,
                (Some(_), None) => false,
            };
            !caught_up
        });
        for (id, known) in &self.items {
            snapshot.items.retain(|i| &i.id != id);
            snapshot.trash.retain(|i| &i.id != id);
            match known {
                Some(item) if item.deleted => snapshot.trash.push(item.clone()),
                Some(item) => snapshot.items.push(item.clone()),
                None => {}
            }
        }

        self.folders.retain(|id, (known, _)| {
            let listed = snapshot.folders.iter().find(|f| &f.id == id);
            listed != known.as_ref()
        });
        for (id, (known, _)) in &self.folders {
            snapshot.folders.retain(|f| &f.id != id);
            if let Some(folder) = known {
                snapshot.folders.push(folder.clone());
            }
        }

        // Items in a folder that no longer exists, as a list that is behind
        // a folder's deletion still shows them, are in no folder.
        let folders: std::collections::HashSet<&str> =
            snapshot.folders.iter().map(|f| f.id.as_str()).collect();
        for item in snapshot.items.iter_mut().chain(snapshot.trash.iter_mut()) {
            if item.folder_id.as_deref().is_some_and(|f| !folders.contains(f)) {
                item.folder_id = None;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use countersign_ipc::summarize;
    use serde_json::json;

    fn item(id: &str, name: &str, revision: &str, deleted: bool) -> ItemSummary {
        summarize(&json!({
            "id": id, "type": 2, "name": name, "folderId": null, "favorite": false,
            "reprompt": 0, "revisionDate": revision,
            "deletedDate": if deleted { json!(revision) } else { json!(null) },
            "secureNote": {"type": 0}
        }))
        .unwrap()
    }

    fn folder(id: &str, name: &str) -> Folder {
        Folder { id: id.into(), name: name.into() }
    }

    fn names(items: &[ItemSummary]) -> Vec<&str> {
        let mut names: Vec<&str> = items.iter().map(|i| i.name.as_str()).collect();
        names.sort_unstable();
        names
    }

    const T1: &str = "2026-11-04T18:34:39.757Z";
    const T2: &str = "2026-11-04T18:34:41.865Z";
    const T3: &str = "2026-11-04T18:40:00.000Z";

    #[test]
    fn an_edit_shows_until_the_list_catches_up() {
        let mut unlisted = Unlisted::default();
        unlisted.record(Known::Item("a".into(), Some(item("a", "new", T2, false))));
        let mut behind = VaultSnapshot { items: vec![item("a", "old", T1, false)], ..Default::default() };
        unlisted.apply(&mut behind);
        assert_eq!(names(&behind.items), ["new"]);
        // A later list that is still behind is corrected again.
        let mut behind = VaultSnapshot { items: vec![item("a", "old", T1, false)], ..Default::default() };
        unlisted.apply(&mut behind);
        assert_eq!(names(&behind.items), ["new"]);
        // Once the list shows the edit, it is forgotten.
        let mut current = VaultSnapshot { items: vec![item("a", "new", T2, false)], ..Default::default() };
        unlisted.apply(&mut current);
        assert!(unlisted.items.is_empty());
        assert_eq!(names(&current.items), ["new"]);
    }

    #[test]
    fn a_change_without_a_new_revision_shows() {
        let mut unlisted = Unlisted::default();
        let mut favorite = item("a", "note", T1, false);
        favorite.favorite = true;
        unlisted.record(Known::Item("a".into(), Some(favorite)));
        let mut behind = VaultSnapshot { items: vec![item("a", "note", T1, false)], ..Default::default() };
        unlisted.apply(&mut behind);
        assert!(behind.items[0].favorite);
        assert_eq!(unlisted.items.len(), 1);
    }

    #[test]
    fn a_newer_revision_from_elsewhere_wins() {
        let mut unlisted = Unlisted::default();
        unlisted.record(Known::Item("a".into(), Some(item("a", "mine", T2, false))));
        let mut synced = VaultSnapshot { items: vec![item("a", "theirs", T3, false)], ..Default::default() };
        unlisted.apply(&mut synced);
        assert_eq!(names(&synced.items), ["theirs"]);
        assert!(unlisted.items.is_empty());
    }

    #[test]
    fn a_new_item_missing_from_the_list_is_added() {
        let mut unlisted = Unlisted::default();
        unlisted.record(Known::Item("b".into(), Some(item("b", "created", T2, false))));
        let mut behind = VaultSnapshot { items: vec![item("a", "other", T1, false)], ..Default::default() };
        unlisted.apply(&mut behind);
        assert_eq!(names(&behind.items), ["created", "other"]);
    }

    #[test]
    fn trashing_and_restoring_move_the_item() {
        let mut unlisted = Unlisted::default();
        unlisted.record(Known::Item("a".into(), Some(item("a", "note", T2, true))));
        let mut behind = VaultSnapshot { items: vec![item("a", "note", T1, false)], ..Default::default() };
        unlisted.apply(&mut behind);
        assert!(behind.items.is_empty());
        assert_eq!(names(&behind.trash), ["note"]);

        unlisted.record(Known::Item("a".into(), Some(item("a", "note", T3, false))));
        let mut behind = VaultSnapshot { trash: vec![item("a", "note", T2, true)], ..Default::default() };
        unlisted.apply(&mut behind);
        assert_eq!(names(&behind.items), ["note"]);
        assert!(behind.trash.is_empty());
    }

    #[test]
    fn an_item_deleted_for_good_stays_gone() {
        let mut unlisted = Unlisted::default();
        unlisted.record(Known::Item("a".into(), None));
        let mut behind = VaultSnapshot { trash: vec![item("a", "note", T1, true)], ..Default::default() };
        unlisted.apply(&mut behind);
        assert!(behind.trash.is_empty());
        assert_eq!(unlisted.items.len(), 1);
        unlisted.apply(&mut VaultSnapshot::default());
        assert!(unlisted.items.is_empty());
    }

    #[test]
    fn folders_show_their_latest_change() {
        let mut unlisted = Unlisted::default();
        unlisted.record(Known::Folder("f".into(), Some(folder("f", "renamed"))));
        unlisted.record(Known::Folder("g".into(), Some(folder("g", "created"))));
        let mut behind = VaultSnapshot { folders: vec![folder("f", "old")], ..Default::default() };
        unlisted.apply(&mut behind);
        let mut folders: Vec<&str> = behind.folders.iter().map(|f| f.name.as_str()).collect();
        folders.sort_unstable();
        assert_eq!(folders, ["created", "renamed"]);
        let mut current = VaultSnapshot {
            folders: vec![folder("f", "renamed"), folder("g", "created")],
            ..Default::default()
        };
        unlisted.apply(&mut current);
        assert!(unlisted.folders.is_empty());
    }

    #[test]
    fn a_deleted_folder_leaves_its_items_in_no_folder() {
        let mut unlisted = Unlisted::default();
        unlisted.record(Known::Folder("f".into(), None));
        let mut filed = item("a", "note", T1, false);
        filed.folder_id = Some("f".into());
        let mut behind =
            VaultSnapshot { items: vec![filed], folders: vec![folder("f", "gone")], ..Default::default() };
        unlisted.apply(&mut behind);
        assert!(behind.folders.is_empty());
        assert_eq!(behind.items[0].folder_id, None);
    }

    #[test]
    fn a_sync_forgets_folders_but_not_items() {
        let mut unlisted = Unlisted::default();
        unlisted.record(Known::Folder("f".into(), Some(folder("f", "mine"))));
        unlisted.record(Known::Item("a".into(), Some(item("a", "new", T2, false))));
        let mark = unlisted.mark();
        // Renamed while the sync runs.
        unlisted.record(Known::Folder("g".into(), Some(folder("g", "during"))));
        unlisted.forget_folders_before(mark);
        let mut synced = VaultSnapshot {
            items: vec![item("a", "old", T1, false)],
            folders: vec![folder("f", "theirs")],
            ..Default::default()
        };
        unlisted.apply(&mut synced);
        assert_eq!(synced.folders, [folder("f", "theirs"), folder("g", "during")]);
        assert_eq!(names(&synced.items), ["new"]);
    }
}
