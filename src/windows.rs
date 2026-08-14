//! The window-list model: fetch, MRU order, workspace grouping, fuzzy filter.

use crate::hypr::ctl;
use fuzzy_matcher::skim::SkimMatcherV2;
use fuzzy_matcher::FuzzyMatcher;

#[derive(Debug, Clone)]
pub struct WinEntry {
    pub address: String,
    pub class: String,
    pub initial_class: String,
    pub title: String,
    pub workspace_id: i64,
    pub workspace_name: String,
    pub focus_history_id: i64,
    pub fullscreen: bool,
    pub floating: bool,
}

pub fn fetch() -> Vec<WinEntry> {
    let Ok(clients) = ctl::clients() else {
        return Vec::new();
    };
    let mut entries: Vec<WinEntry> = clients
        .into_iter()
        .filter(|c| {
            c.mapped && !c.hidden && !c.class.starts_with("sidetab") && c.workspace.id > 0
        })
        .map(|c| WinEntry {
            address: c.address,
            class: c.class,
            initial_class: c.initial_class,
            title: c.title,
            workspace_id: c.workspace.id,
            workspace_name: c.workspace.name,
            focus_history_id: c.focus_history_id,
            // bit 2 = fullscreen; bit 1 alone is only maximized
            fullscreen: c.fullscreen & 2 != 0,
            floating: c.floating,
        })
        .collect();
    entries.sort_by_key(|e| e.focus_history_id);
    entries
}

/// A section in the panel, macOS-Contexts style: "Pinned" apps first, then
/// "Full Screen", then one group per workspace holding its tiled windows,
/// then "Floating". `rows` index into the entries slice passed to `group`.
/// Display order within a group is most-recently-used.
#[derive(Debug, Clone)]
pub struct Group {
    pub label: String,
    pub rows: Vec<usize>,
}

pub fn group(entries: &[WinEntry], pinned: &[String]) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();
    let is_pinned =
        |e: &WinEntry| pinned.iter().any(|p| p == &e.class || p == &e.initial_class);
    let pinned_rows: Vec<usize> = (0..entries.len())
        .filter(|&i| is_pinned(&entries[i]))
        .collect();
    if !pinned_rows.is_empty() {
        groups.push(Group {
            label: "Pinned".to_string(),
            rows: pinned_rows,
        });
    }
    let fullscreen: Vec<usize> = (0..entries.len())
        .filter(|&i| entries[i].fullscreen && !is_pinned(&entries[i]))
        .collect();
    if !fullscreen.is_empty() {
        groups.push(Group {
            label: "Full Screen".to_string(),
            rows: fullscreen,
        });
    }
    let is_tiled = |e: &WinEntry| !e.fullscreen && !e.floating && !is_pinned(e);
    let mut ws_ids: Vec<i64> = entries
        .iter()
        .filter(|e| is_tiled(e))
        .map(|e| e.workspace_id)
        .collect();
    ws_ids.sort_unstable();
    ws_ids.dedup();
    for ws in ws_ids {
        let rows: Vec<usize> = (0..entries.len())
            .filter(|&i| is_tiled(&entries[i]) && entries[i].workspace_id == ws)
            .collect();
        let label = match entries[rows[0]].workspace_name.parse::<i64>() {
            Ok(_) => format!("Workspace {ws}"),
            Err(_) => entries[rows[0]].workspace_name.clone(),
        };
        groups.push(Group { label, rows });
    }
    let floating: Vec<usize> = (0..entries.len())
        .filter(|&i| {
            !entries[i].fullscreen && entries[i].floating && !is_pinned(&entries[i])
        })
        .collect();
    if !floating.is_empty() {
        groups.push(Group {
            label: "Floating".to_string(),
            rows: floating,
        });
    }
    groups
}

/// Flat display order (entry indices) for selection cycling and digit hints.
pub fn display_order(groups: &[Group]) -> Vec<usize> {
    groups.iter().flat_map(|g| g.rows.iter().copied()).collect()
}

/// Fuzzy-filtered entry indices, best match first.
pub fn filter(entries: &[WinEntry], query: &str) -> Vec<usize> {
    let matcher = SkimMatcherV2::default();
    let mut scored: Vec<(i64, usize)> = entries
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            let haystack = format!("{} {} {}", e.class, e.initial_class, e.title);
            matcher.fuzzy_match(&haystack, query).map(|s| (s, i))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    scored.into_iter().map(|(_, i)| i).collect()
}

/// Nice app display name from a window class ("org.mozilla.firefox" -> "Firefox").
pub fn app_name(class: &str) -> String {
    let last = class.rsplit('.').next().unwrap_or(class);
    let mut chars = last.chars();
    match chars.next() {
        Some(f) => f.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}
