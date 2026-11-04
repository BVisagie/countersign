//! Changes the CLI's lists do not show yet.
//!
//! `bw serve` 2026.10.1 often answers a list from before the latest change:
//! an edited item keeps its old name, a renamed folder its old name, a new
//! item or folder is missing. The list stays behind until a later change or
//! a sync. So the daemon keeps what each change left and lays it over every
//! list until the list shows it, or until a sync that started after the
//! change has finished. Changes from elsewhere arrive only through a sync,
//! so after one the CLI's lists are the truth again.

use std::collections::{HashMap, HashSet};

use countersign_ipc::{Folder, ItemSummary, VaultSnapshot};

#[derive(Debug, Default)]
pub(crate) struct Unlisted {
    /// Items as a change left them, `None` once deleted for good, with the
    /// number of the change.
    items: HashMap<String, (Option<ItemSummary>, u64)>,
    /// Folders as a change left them, `None` once deleted.
    folders: HashMap<String, (Option<Folder>, u64)>,
    /// Changes recorded so far.
    changes: u64,
}

impl Unlisted {
    /// An item as a change returned it, or `None` once deleted for good.
    pub(crate) fn item(&mut self, id: String, item: Option<ItemSummary>) {
        self.changes += 1;
        self.items.insert(id, (item, self.changes));
    }

    /// An item changed in a way the change did not return, such as moving it
    /// to the trash: the latest known state, from an earlier change or else
    /// from the index, with `change` applied.
    pub(crate) fn derive(
        &mut self,
        id: &str,
        indexed: Option<&ItemSummary>,
        change: impl FnOnce(&mut ItemSummary),
    ) {
        let mut item = match self.items.get(id) {
            Some((Some(item), _)) => item.clone(),
            Some((None, _)) => return,
            None => match indexed {
                Some(item) => item.clone(),
                None => return,
            },
        };
        change(&mut item);
        self.item(id.to_owned(), Some(item));
    }

    /// A folder as a change returned it, or `None` once deleted. Deleting a
    /// folder leaves its items in no folder.
    pub(crate) fn folder(&mut self, id: String, folder: Option<Folder>) {
        self.changes += 1;
        if folder.is_none() {
            for (item, _) in self.items.values_mut() {
                if let Some(item) = item.as_mut().filter(|i| i.folder_id.as_deref() == Some(&id)) {
                    item.folder_id = None;
                }
            }
        }
        self.folders.insert(id, (folder, self.changes));
    }

    /// Marks the start of a sync, for `forget_before`.
    pub(crate) fn mark(&self) -> u64 {
        self.changes
    }

    /// Forgets the changes recorded before a sync started, once it has
    /// finished. A change made during the sync stays.
    pub(crate) fn forget_before(&mut self, mark: u64) {
        self.items.retain(|_, (_, change)| *change > mark);
        self.folders.retain(|_, (_, change)| *change > mark);
    }

    /// Corrects a snapshot built from the CLI's lists, and forgets each
    /// change the lists now show.
    pub(crate) fn apply(&mut self, snapshot: &mut VaultSnapshot) {
        self.folders.retain(|id, (known, _)| snapshot.folders.iter().find(|f| &f.id == id) != known.as_ref());
        for (id, (known, _)) in &self.folders {
            snapshot.folders.retain(|f| &f.id != id);
            if let Some(folder) = known {
                snapshot.folders.push(folder.clone());
            }
        }

        // An item in a folder that no longer exists, as a list behind a
        // folder's deletion still shows it, is in no folder.
        let folders: HashSet<String> = snapshot.folders.iter().map(|f| f.id.clone()).collect();
        let dangling = |item: &mut ItemSummary| {
            if item.folder_id.as_ref().is_some_and(|f| !folders.contains(f)) {
                item.folder_id = None;
            }
        };
        snapshot.items.iter_mut().chain(snapshot.trash.iter_mut()).for_each(dangling);
        self.items.values_mut().filter_map(|(item, _)| item.as_mut()).for_each(dangling);

        self.items.retain(|id, (known, _)| {
            let listed = snapshot.items.iter().chain(&snapshot.trash).find(|i| &i.id == id);
            let caught_up = match (known, listed) {
                (None, listed) => listed.is_none(),
                (Some(known), Some(listed)) => same(known, listed),
                (Some(_), None) => false,
            };
            !caught_up
        });
        for (id, (known, _)) in &self.items {
            snapshot.items.retain(|i| &i.id != id);
            snapshot.trash.retain(|i| &i.id != id);
            match known {
                Some(item) if item.deleted => snapshot.trash.push(item.clone()),
                Some(item) => snapshot.items.push(item.clone()),
                None => {}
            }
        }
    }
}

/// Whether a list shows an item as a change left it. The revision is left
/// out: a change derived from the index cannot know the new one.
fn same(known: &ItemSummary, listed: &ItemSummary) -> bool {
    let mut listed = listed.clone();
    listed.revision.clone_from(&known.revision);
    &listed == known
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

    fn listing(items: Vec<ItemSummary>) -> VaultSnapshot {
        VaultSnapshot { items, ..Default::default() }
    }

    const T1: &str = "2026-11-04T18:34:39.757Z";
    const T2: &str = "2026-11-04T18:34:41.865Z";
    const T3: &str = "2026-11-04T18:40:00.000Z";

    #[test]
    fn an_edit_shows_until_the_list_catches_up() {
        let mut unlisted = Unlisted::default();
        unlisted.item("a".into(), Some(item("a", "new", T2, false)));
        for _ in 0..2 {
            let mut behind = listing(vec![item("a", "old", T1, false)]);
            unlisted.apply(&mut behind);
            assert_eq!(names(&behind.items), ["new"]);
        }
        let mut current = listing(vec![item("a", "new", T2, false)]);
        unlisted.apply(&mut current);
        assert!(unlisted.items.is_empty());
        assert_eq!(names(&current.items), ["new"]);
    }

    #[test]
    fn a_change_without_a_new_revision_shows() {
        let mut unlisted = Unlisted::default();
        let mut favorite = item("a", "note", T1, false);
        favorite.favorite = true;
        unlisted.item("a".into(), Some(favorite));
        let mut behind = listing(vec![item("a", "note", T1, false)]);
        unlisted.apply(&mut behind);
        assert!(behind.items[0].favorite);
        assert_eq!(unlisted.items.len(), 1);
    }

    #[test]
    fn a_new_item_missing_from_the_list_is_added() {
        let mut unlisted = Unlisted::default();
        unlisted.item("b".into(), Some(item("b", "created", T2, false)));
        let mut behind = listing(vec![item("a", "other", T1, false)]);
        unlisted.apply(&mut behind);
        assert_eq!(names(&behind.items), ["created", "other"]);
    }

    #[test]
    fn trashing_and_restoring_move_the_item() {
        let mut unlisted = Unlisted::default();
        let indexed = item("a", "note", T1, false);
        unlisted.derive("a", Some(&indexed), |i| i.deleted = true);
        let mut behind = listing(vec![indexed.clone()]);
        unlisted.apply(&mut behind);
        assert!(behind.items.is_empty());
        assert_eq!(names(&behind.trash), ["note"]);

        unlisted.derive("a", None, |i| i.deleted = false);
        let mut behind = VaultSnapshot { trash: vec![item("a", "note", T1, true)], ..Default::default() };
        unlisted.apply(&mut behind);
        assert_eq!(names(&behind.items), ["note"]);
        assert!(behind.trash.is_empty());
        // Restoring gives the item a new revision, which the derived entry
        // cannot know; the list showing it restored is enough.
        let mut current = listing(vec![item("a", "note", T2, false)]);
        unlisted.apply(&mut current);
        assert!(unlisted.items.is_empty());
    }

    #[test]
    fn trashing_after_a_restore_stays_in_the_trash() {
        let mut unlisted = Unlisted::default();
        unlisted.derive("a", Some(&item("a", "note", T1, true)), |i| i.deleted = false);
        unlisted.derive("a", None, |i| i.deleted = true);
        // One change behind: restored, with the restore's newer revision.
        let mut behind = listing(vec![item("a", "note", T2, false)]);
        unlisted.apply(&mut behind);
        assert!(behind.items.is_empty());
        assert_eq!(names(&behind.trash), ["note"]);
    }

    #[test]
    fn a_derived_change_builds_on_the_latest_change() {
        let mut unlisted = Unlisted::default();
        let mut favorite = item("a", "note", T1, false);
        favorite.favorite = true;
        unlisted.item("a".into(), Some(favorite));
        // The index still shows it without the favorite.
        unlisted.derive("a", Some(&item("a", "note", T1, false)), |i| i.deleted = true);
        let mut behind = listing(vec![item("a", "note", T1, false)]);
        unlisted.apply(&mut behind);
        assert!(behind.trash[0].favorite);
    }

    #[test]
    fn an_item_deleted_for_good_stays_gone() {
        let mut unlisted = Unlisted::default();
        unlisted.item("a".into(), None);
        unlisted.derive("a", Some(&item("a", "note", T1, true)), |i| i.deleted = false);
        let mut behind = VaultSnapshot { trash: vec![item("a", "note", T1, true)], ..Default::default() };
        unlisted.apply(&mut behind);
        assert!(behind.trash.is_empty() && behind.items.is_empty());
        unlisted.apply(&mut VaultSnapshot::default());
        assert!(unlisted.items.is_empty());
    }

    #[test]
    fn folders_show_their_latest_change() {
        let mut unlisted = Unlisted::default();
        unlisted.folder("f".into(), Some(folder("f", "renamed")));
        unlisted.folder("g".into(), Some(folder("g", "created")));
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
        let mut filed = item("a", "note", T1, false);
        filed.folder_id = Some("f".into());
        unlisted.item("a".into(), Some(filed.clone()));
        unlisted.folder("f".into(), None);
        let mut behind =
            VaultSnapshot { items: vec![filed], folders: vec![folder("f", "gone")], ..Default::default() };
        unlisted.apply(&mut behind);
        assert!(behind.folders.is_empty());
        assert_eq!(behind.items[0].folder_id, None);
        // A list that is current retires both.
        let mut current = listing(vec![item("a", "note", T1, false)]);
        unlisted.apply(&mut current);
        assert!(unlisted.items.is_empty() && unlisted.folders.is_empty());
    }

    #[test]
    fn a_sync_retires_what_came_before_it() {
        let mut unlisted = Unlisted::default();
        let mut favorite = item("a", "note", T1, false);
        favorite.favorite = true;
        unlisted.item("a".into(), Some(favorite));
        unlisted.item("b".into(), Some(item("b", "deleted elsewhere", T1, false)));
        unlisted.folder("f".into(), Some(folder("f", "mine")));
        let mark = unlisted.mark();
        // Changed while the sync runs.
        unlisted.item("c".into(), Some(item("c", "during", T3, false)));
        unlisted.forget_before(mark);
        let mut synced = VaultSnapshot {
            items: vec![item("a", "note", T1, false)],
            folders: vec![folder("f", "theirs")],
            ..Default::default()
        };
        unlisted.apply(&mut synced);
        assert!(!synced.items.iter().any(|i| i.favorite));
        assert_eq!(names(&synced.items), ["during", "note"]);
        assert_eq!(synced.folders, [folder("f", "theirs")]);
    }
}
