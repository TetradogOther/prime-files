//! Tags on folders and files, as the Files app has them: a coloured name
//! put on anything, listed in the sidebar; clicking a tag lists everything
//! that carries it, from wherever it is.
//!
//! One set for the whole app, held as a GPUI global and written, debounced,
//! to the same redb store as the tabs. Items are remembered by path, without
//! case (Windows paths ignore it). A copy of which paths carry each tag is
//! kept where the listing thread can read it: a tag's "folder" is read there
//! like any other (`navigation::read_folder`), off the UI thread, since each
//! item is a disk access and some may be on a share that does not answer.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{App, AppContext as _, BorrowAppContext, Global, Task};
use nohrs_core::telemetry::LogErr;
use nohrs_services::fs::listing::FileEntryDto;
use nohrs_store::{KvKey, KvStore, kv_key};
use serde::{Deserialize, Serialize};

const TAGS_KEY: KvKey = kv_key!("explorer.tags");

/// Quiet time before the tags are written: ticking a tag on fifty files is
/// one write.
const SAVE_DEBOUNCE: Duration = Duration::from_millis(600);

/// The address a pane shows a tag's items at: this, then the tag's id.
pub(crate) const TAG_PREFIX: &str = "tag:";

/// The colours offered for a tag, in order; a new tag takes the next one.
pub(crate) const COLORS: [(u32, &str); 8] = [
    (0xE5484D, nohrs_core::k!("Vermelho")),
    (0xF76B15, nohrs_core::k!("Laranja")),
    (0xFFC53D, nohrs_core::k!("Amarelo")),
    (0x46A758, nohrs_core::k!("Verde")),
    (0x12A594, nohrs_core::k!("Turquesa")),
    (0x3E63DD, nohrs_core::k!("Azul")),
    (0x8E4EC6, nohrs_core::k!("Roxo")),
    (0xD6409F, nohrs_core::k!("Rosa")),
];

/// A tag: its name and colour (`0xRRGGBB`).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Tag {
    pub id: u32,
    pub name: String,
    pub color: u32,
}

#[derive(Default, Serialize, Deserialize)]
struct Stored {
    tags: Vec<Tag>,
    /// Each item as last written, with its tags' ids.
    items: Vec<(String, Vec<u32>)>,
    next_id: u32,
}

/// The app's tags.
#[derive(Default)]
pub(crate) struct Tags {
    tags: Vec<Tag>,
    /// By [`item_key`]: the path as written and its tags' ids.
    items: HashMap<String, (String, Vec<u32>)>,
    next_id: u32,
    store: Option<Arc<dyn KvStore>>,
    save: Option<Task<()>>,
}

impl Global for Tags {}

// Which paths carry each tag, for the listing thread.
static TAGGED: Mutex<Option<HashMap<u32, Vec<String>>>> = Mutex::new(None);

// `layout.sidebar_tags`: whether the sidebar shows the tags' group.
static IN_SIDEBAR: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

/// Shows or hides the tags' group in the sidebar. Whether it changed.
pub(crate) fn set_in_sidebar(shown: bool) -> bool {
    IN_SIDEBAR.swap(shown, std::sync::atomic::Ordering::Relaxed) != shown
}

/// Whether the sidebar shows the tags' group.
pub(crate) fn in_sidebar() -> bool {
    IN_SIDEBAR.load(std::sync::atomic::Ordering::Relaxed)
}

/// The same item however its path was spelled.
fn item_key(path: &str) -> String {
    path.trim_end_matches(['\\', '/'])
        .replace('/', "\\")
        .to_lowercase()
}

/// The tag a pane's address names, if it is a tag's.
pub(crate) fn tag_of_location(location: &str) -> Option<u32> {
    location.strip_prefix(TAG_PREFIX)?.parse().ok()
}

/// The address that lists the items of tag `id`.
pub(crate) fn location(id: u32) -> String {
    format!("{TAG_PREFIX}{id}")
}

/// Reads the items of tag `id` from disk, each a row of the listing; one no
/// longer there is left out. Called from the listing thread.
#[allow(clippy::disallowed_methods)]
pub(crate) fn read_items(id: u32) -> Vec<FileEntryDto> {
    let paths = TAGGED
        .lock()
        .ok()
        .and_then(|tagged| tagged.as_ref().and_then(|tagged| tagged.get(&id).cloned()))
        .unwrap_or_default();
    paths
        .into_iter()
        .filter_map(|path| {
            let metadata = std::fs::metadata(&path).ok()?;
            let name = std::path::Path::new(&path)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| path.clone());
            let modified = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|age| age.as_secs())
                .unwrap_or(0);
            Some(FileEntryDto {
                name,
                kind: if metadata.is_dir() { "dir" } else { "file" }.to_string(),
                size: if metadata.is_dir() { 0 } else { metadata.len() },
                modified,
                path,
                attributes: 0,
            })
        })
        .collect()
}

impl Tags {
    /// Loads the tags from `store` and makes them the app's. A first start
    /// gets four to begin with, as the Files app does; they can be renamed,
    /// recoloured or deleted like any.
    pub(crate) fn install(store: Option<Arc<dyn KvStore>>, cx: &mut App) {
        if cx.has_global::<Self>() {
            return;
        }
        let stored = store
            .as_ref()
            .and_then(|store| store.get(&TAGS_KEY).log_err().flatten())
            .and_then(|bytes| serde_json::from_slice::<Stored>(&bytes).log_err());
        let mut tags = Self {
            store,
            ..Default::default()
        };
        match stored {
            Some(stored) => {
                tags.tags = stored.tags;
                tags.next_id = stored.next_id;
                for (path, ids) in stored.items {
                    tags.items.insert(item_key(&path), (path, ids));
                }
            }
            None => {
                for (name, color) in [
                    (nohrs_core::t!("Importante"), COLORS[0].0),
                    (nohrs_core::t!("Trabalho"), COLORS[5].0),
                    (nohrs_core::t!("Pessoal"), COLORS[3].0),
                    (nohrs_core::t!("Projetos"), COLORS[1].0),
                ] {
                    tags.push_tag(name.to_string(), color);
                }
            }
        }
        tags.publish();
        cx.set_global(tags);
    }

    fn push_tag(&mut self, name: String, color: u32) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        self.tags.push(Tag { id, name, color });
        id
    }

    // Hands the listing thread the paths of each tag.
    fn publish(&self) {
        let mut tagged: HashMap<u32, Vec<String>> = HashMap::new();
        for (path, ids) in self.items.values() {
            for id in ids {
                tagged.entry(*id).or_default().push(path.clone());
            }
        }
        match TAGGED.lock() {
            Ok(mut slot) => *slot = Some(tagged),
            Err(error) => tracing::error!("tag index poisoned: {error}"),
        }
    }

    /// Every tag, in the order they were made.
    pub(crate) fn all(cx: &App) -> Vec<Tag> {
        cx.try_global::<Self>()
            .map(|tags| tags.tags.clone())
            .unwrap_or_default()
    }

    /// The tag `id`, if it exists.
    pub(crate) fn get(id: u32, cx: &App) -> Option<Tag> {
        cx.try_global::<Self>()?
            .tags
            .iter()
            .find(|tag| tag.id == id)
            .cloned()
    }

    /// The colours of the tags on `path`, for the dots beside its name.
    /// Nothing is looked up while no item has a tag.
    pub(crate) fn colors_of(path: &str, cx: &App) -> Vec<u32> {
        let Some(tags) = cx.try_global::<Self>() else {
            return Vec::new();
        };
        if tags.items.is_empty() {
            return Vec::new();
        }
        let Some((_, ids)) = tags.items.get(&item_key(path)) else {
            return Vec::new();
        };
        ids.iter()
            .filter_map(|id| {
                tags.tags
                    .iter()
                    .find(|tag| tag.id == *id)
                    .map(|tag| tag.color)
            })
            .collect()
    }

    /// How many items carry each tag, by id.
    pub(crate) fn counts(cx: &App) -> HashMap<u32, usize> {
        let mut counts = HashMap::new();
        if let Some(tags) = cx.try_global::<Self>() {
            for (_, ids) in tags.items.values() {
                for id in ids {
                    *counts.entry(*id).or_insert(0) += 1;
                }
            }
        }
        counts
    }

    /// Whether every one of `paths` carries tag `id` (what its tick shows).
    pub(crate) fn all_have(paths: &[String], id: u32, cx: &App) -> bool {
        let Some(tags) = cx.try_global::<Self>() else {
            return false;
        };
        !paths.is_empty()
            && paths.iter().all(|path| {
                tags.items
                    .get(&item_key(path))
                    .is_some_and(|(_, ids)| ids.contains(&id))
            })
    }

    /// Puts tag `id` on every one of `paths`, or takes it off all of them
    /// when they all have it already, as a tick in a menu does.
    pub(crate) fn toggle(paths: &[String], id: u32, cx: &mut App) {
        let remove = Self::all_have(paths, id, cx);
        Self::change(cx, |tags| {
            for path in paths {
                let key = item_key(path);
                if remove {
                    if let Some((_, ids)) = tags.items.get_mut(&key) {
                        ids.retain(|tag| *tag != id);
                        if ids.is_empty() {
                            tags.items.remove(&key);
                        }
                    }
                } else {
                    let (_, ids) = tags
                        .items
                        .entry(key)
                        .or_insert_with(|| (path.clone(), Vec::new()));
                    if !ids.contains(&id) {
                        ids.push(id);
                    }
                }
            }
        });
    }

    /// Puts tag `id` on every one of `paths` (a drop on the tag: it adds,
    /// never takes off). How many did not have it.
    pub(crate) fn add(paths: &[String], id: u32, cx: &mut App) -> usize {
        let mut added = 0;
        Self::change(cx, |tags| {
            if !tags.tags.iter().any(|tag| tag.id == id) {
                return;
            }
            for path in paths {
                let (_, ids) = tags
                    .items
                    .entry(item_key(path))
                    .or_insert_with(|| (path.clone(), Vec::new()));
                if !ids.contains(&id) {
                    ids.push(id);
                    added += 1;
                }
            }
        });
        added
    }

    /// Takes tag `id` off `paths` ("Remove from this tag").
    pub(crate) fn remove(paths: &[String], id: u32, cx: &mut App) {
        Self::change(cx, |tags| {
            for path in paths {
                let key = item_key(path);
                if let Some((_, ids)) = tags.items.get_mut(&key) {
                    ids.retain(|tag| *tag != id);
                    if ids.is_empty() {
                        tags.items.remove(&key);
                    }
                }
            }
        });
    }

    /// Takes every tag off `paths` (Alt+0).
    pub(crate) fn clear(paths: &[String], cx: &mut App) {
        Self::change(cx, |tags| {
            for path in paths {
                tags.items.remove(&item_key(path));
            }
        });
    }

    /// The `index`th tag (0-based) in the sidebar's order, for Alt+1….
    pub(crate) fn nth(index: usize, cx: &App) -> Option<Tag> {
        cx.try_global::<Self>()?.tags.get(index).cloned()
    }

    /// Makes a tag with the next colour and puts it on `paths`. Returns its id.
    pub(crate) fn create(name: String, paths: &[String], cx: &mut App) -> u32 {
        let mut created = 0;
        Self::change(cx, |tags| {
            let color = COLORS[tags.tags.len() % COLORS.len()].0;
            created = tags.push_tag(name, color);
        });
        if !paths.is_empty() {
            Self::toggle(paths, created, cx);
        }
        created
    }

    pub(crate) fn rename(id: u32, name: String, cx: &mut App) {
        let name = name.trim().to_string();
        if name.is_empty() {
            return;
        }
        Self::change(cx, |tags| {
            if let Some(tag) = tags.tags.iter_mut().find(|tag| tag.id == id) {
                tag.name = name;
            }
        });
    }

    pub(crate) fn recolor(id: u32, color: u32, cx: &mut App) {
        Self::change(cx, |tags| {
            if let Some(tag) = tags.tags.iter_mut().find(|tag| tag.id == id) {
                tag.color = color;
            }
        });
    }

    /// Deletes tag `id` and takes it off every item.
    pub(crate) fn delete(id: u32, cx: &mut App) {
        Self::change(cx, |tags| {
            tags.tags.retain(|tag| tag.id != id);
            tags.items.retain(|_, (_, ids)| {
                ids.retain(|tag| *tag != id);
                !ids.is_empty()
            });
        });
    }

    /// A tagged item was renamed or moved here: its tags follow it.
    pub(crate) fn moved(from: &str, to: &str, cx: &mut App) {
        let Some(tags) = cx.try_global::<Self>() else {
            return;
        };
        if !tags.items.contains_key(&item_key(from)) {
            return;
        }
        let (from, to) = (from.to_string(), to.to_string());
        Self::change(cx, move |tags| {
            if let Some((_, ids)) = tags.items.remove(&item_key(&from)) {
                tags.items.insert(item_key(&to), (to.clone(), ids));
            }
        });
    }

    // Applies `edit`, republishes the index for the listing thread and saves
    // shortly after.
    fn change(cx: &mut App, edit: impl FnOnce(&mut Self)) {
        if !cx.has_global::<Self>() {
            return;
        }
        cx.update_global(|tags: &mut Self, cx| {
            edit(tags);
            tags.publish();
            if tags.store.is_none() {
                return;
            }
            tags.save = Some(cx.spawn(async move |cx| {
                cx.background_executor().timer(SAVE_DEBOUNCE).await;
                let prepared = cx
                    .update(|cx| {
                        let tags = cx.global::<Self>();
                        let stored = Stored {
                            tags: tags.tags.clone(),
                            items: tags.items.values().cloned().collect(),
                            next_id: tags.next_id,
                        };
                        let bytes = serde_json::to_vec(&stored).log_err()?;
                        Some((tags.store.clone()?, bytes))
                    })
                    .ok()
                    .flatten();
                let Some((store, bytes)) = prepared else {
                    return;
                };
                cx.background_spawn(async move {
                    store.put(&TAGS_KEY, &bytes).log_err();
                })
                .await;
            }));
        });
        cx.refresh_windows();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn a_tag_lists_what_carries_it_and_follows_a_rename(cx: &mut gpui::TestAppContext) {
        let folder = tempfile::tempdir().expect("temp folder");
        let one = folder.path().join("one.txt");
        let two = folder.path().join("Two");
        std::fs::write(&one, "1").expect("write");
        std::fs::create_dir(&two).expect("folder");
        let (one, two) = (
            one.to_string_lossy().into_owned(),
            two.to_string_lossy().into_owned(),
        );
        cx.update(|cx| {
            Tags::install(None, cx);
            let id = Tags::create("Edição".into(), &[one.clone(), two.clone()], cx);
            let mut names: Vec<String> =
                read_items(id).into_iter().map(|entry| entry.name).collect();
            names.sort();
            assert_eq!(names, ["Two", "one.txt"]);
            // Another spelling of the same path is the same item.
            assert!(Tags::all_have(&[one.to_uppercase()], id, cx));
            // Ticked on both, the tick takes it off both.
            Tags::toggle(&[one.clone(), two.clone()], id, cx);
            assert!(read_items(id).is_empty());
            Tags::toggle(&[one.clone()], id, cx);
            let renamed = folder
                .path()
                .join("renamed.txt")
                .to_string_lossy()
                .into_owned();
            std::fs::rename(&one, &renamed).expect("rename");
            Tags::moved(&one, &renamed, cx);
            let names: Vec<String> = read_items(id).into_iter().map(|entry| entry.name).collect();
            assert_eq!(names, ["renamed.txt"]);
            Tags::delete(id, cx);
            assert!(Tags::colors_of(&renamed, cx).is_empty());
        });
    }
}
