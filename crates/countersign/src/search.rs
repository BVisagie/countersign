//! Ranks vault items for a search query.
//!
//! Matching uses nucleo's fuzzy matcher, the one Helix uses. Names count
//! most, then websites, then usernames. Prefix matches, favorites and
//! recently used items get a small lift so `git` puts GitHub first.

use countersign_ipc::ItemSummary;
use nucleo_matcher::pattern::{AtomKind, CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

pub struct Searcher {
    matcher: Matcher,
    buf: Vec<char>,
}

impl Default for Searcher {
    fn default() -> Self {
        Searcher { matcher: Matcher::new(Config::DEFAULT), buf: Vec::new() }
    }
}

impl Searcher {
    /// Indices into `items`, best match first. An empty query keeps the
    /// order given, with recent items first when `recent` is not empty.
    pub fn rank(&mut self, items: &[&ItemSummary], query: &str, recent: &[String]) -> Vec<usize> {
        let query = query.trim();
        if query.is_empty() {
            let mut order: Vec<usize> = (0..items.len()).collect();
            if !recent.is_empty() {
                let rank = |i: usize| recent.iter().position(|r| *r == items[i].id).unwrap_or(usize::MAX);
                order.sort_by_key(|&i| rank(i));
            }
            return order;
        }
        let pattern = Pattern::new(query, CaseMatching::Ignore, Normalization::Smart, AtomKind::Fuzzy);
        let lower = query.to_lowercase();
        let mut scored: Vec<(u32, usize)> = items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| {
                let name = self.score(&pattern, &item.name).map(|s| s * 3);
                let host = item.hosts.iter().filter_map(|h| self.score(&pattern, h)).max().map(|s| s * 2);
                let user = item.subtitle.as_deref().and_then(|u| self.score(&pattern, u));
                let mut best = [name, host, user].into_iter().flatten().max()?;
                let name_lower = item.name.to_lowercase();
                if name_lower.starts_with(&lower) {
                    best += 400;
                } else if name_lower.split_whitespace().any(|w| w.starts_with(&lower)) {
                    best += 200;
                }
                if item.hosts.iter().any(|h| h.starts_with(&lower) || h.contains(&format!(".{lower}"))) {
                    best += 120;
                }
                if item.favorite {
                    best += 40;
                }
                if let Some(pos) = recent.iter().position(|r| *r == item.id) {
                    best += 60u32.saturating_sub(pos as u32 * 6);
                }
                Some((best, i))
            })
            .collect();
        scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        scored.into_iter().map(|(_, i)| i).collect()
    }

    fn score(&mut self, pattern: &Pattern, text: &str) -> Option<u32> {
        pattern.score(Utf32Str::new(text, &mut self.buf), &mut self.matcher)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use countersign_ipc::ItemKind;

    fn item(id: &str, name: &str, host: &str, user: &str) -> ItemSummary {
        ItemSummary {
            id: id.into(),
            kind: ItemKind::Login,
            name: name.into(),
            subtitle: Some(user.into()),
            username: Some(user.into()),
            hosts: vec![host.into()],
            first_uri: None,
            favorite: false,
            folder_id: None,
            organization_id: None,
            collection_ids: vec![],
            has_password: true,
            has_totp: false,
            has_passkey: false,
            attachments: 0,
            reprompt: false,
            deleted: false,
            archived: false,
            revision: None,
        }
    }

    #[test]
    fn prefers_name_prefixes() {
        let items = [
            item("1", "DigitalOcean", "cloud.digitalocean.com", "me"),
            item("2", "GitHub", "github.com", "me"),
            item("3", "Gitea", "git.example.org", "me"),
            item("4", "Spotify", "open.spotify.com", "git-user"),
        ];
        let refs: Vec<&ItemSummary> = items.iter().collect();
        let mut searcher = Searcher::default();
        let order = searcher.rank(&refs, "git", &[]);
        let mut first_two = order[..2].to_vec();
        first_two.sort();
        assert_eq!(first_two, vec![1, 2], "GitHub and Gitea lead");
        assert!(order.contains(&3), "a username match still appears");
        assert!(searcher.rank(&refs, "zzzz", &[]).is_empty());
    }

    #[test]
    fn empty_query_puts_recent_first() {
        let items =
            [item("1", "A", "a.com", "u"), item("2", "B", "b.com", "u"), item("3", "C", "c.com", "u")];
        let refs: Vec<&ItemSummary> = items.iter().collect();
        let order = Searcher::default().rank(&refs, "", &["3".into()]);
        assert_eq!(order, vec![2, 0, 1]);
    }

    /// Run with `cargo test --release -p countersign -- --ignored search_speed --nocapture`.
    #[test]
    #[ignore = "timing; run in release"]
    fn search_speed() {
        let words = ["git", "mail", "bank", "cloud", "shop", "news", "forum", "photo", "music", "video"];
        let items: Vec<ItemSummary> = (0..5000)
            .map(|i| {
                let w = words[i % words.len()];
                item(
                    &i.to_string(),
                    &format!("{w} account {i}"),
                    &format!("{w}{i}.example.com"),
                    &format!("user{i}@example.test"),
                )
            })
            .collect();
        let refs: Vec<&ItemSummary> = items.iter().collect();
        let mut searcher = Searcher::default();
        let mut worst = std::time::Duration::ZERO;
        let mut query = String::new();
        for c in "git account 42".chars() {
            query.push(c);
            let start = std::time::Instant::now();
            let order = searcher.rank(&refs, &query, &[]);
            let took = start.elapsed();
            worst = worst.max(took);
            println!("{query:>16}: {:>5} results in {took:?}", order.len());
        }
        println!("worst keystroke over 5000 items: {worst:?}");
        assert!(worst < std::time::Duration::from_millis(16));
    }
}
