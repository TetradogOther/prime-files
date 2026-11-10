//! The folder on screen follows the disk: a file another program saves,
//! downloads, deletes or renames there shows up (or goes) without F5.
//!
//! Until 05/11/2026 the listing was read on entering a folder, on F5 and
//! after Prime Files' own operations only, so a file saved by an editor or a
//! browser did not appear — a person who tried it said that alone made them
//! leave the explorer they had used before. Explorer itself watches the
//! folders it shows; so does this, with the system's change notifications
//! (`ReadDirectoryChangesW`, through `notify`), not by polling.
//!
//! Only the folder on screen and the Tree view's open folders are watched,
//! not their subfolders: what is not shown needs no refresh. A save is a
//! burst of events (create, write, rename into place), and a download
//! writes for minutes, so events are gathered for [`SETTLE`] and refreshes
//! kept [`SPACING`] apart; each one is the same refresh as F5, which keeps
//! selection, scroll, open folders and preview.

use std::path::Path;
use std::time::{Duration, Instant};

use gpui::{Context, Task, WeakEntity};
use notify::{EventKind, RecursiveMode, Watcher as _};

use super::ExplorerPane;

/// How long a burst of events is gathered before the folder is read.
const SETTLE: Duration = Duration::from_millis(200);
/// The least time between two refreshes of a folder that keeps changing.
const SPACING: Duration = Duration::from_millis(800);
/// How often a refresh put off (an open rename field, a search on screen) is
/// tried again.
const RETRY: Duration = Duration::from_secs(1);
/// How often a folder whose drive sends no notifications is looked at again.
const POLL: Duration = Duration::from_secs(3);

/// The watch on a pane's folders; dropping it stops it.
pub(crate) struct FolderWatch {
    folders: Vec<String>,
    _watcher: notify::RecommendedWatcher,
    _task: Task<()>,
    _poll: Option<Task<()>>,
}

impl ExplorerPane {
    /// Watches what is on screen now: the folder, and the folders open in
    /// the Tree view. Called after each listing; a no-op when that set did
    /// not change.
    pub(crate) fn watch_listed_folders(&mut self, cx: &mut Context<Self>) {
        let mut folders: Vec<String> = std::iter::once(self.cwd.clone())
            .chain(self.tree.expanded.iter().cloned())
            // "Este Computador" and other shell places are no folder on disk.
            .filter(|folder| Path::new(folder).is_dir())
            .collect();
        folders.sort();
        folders.dedup();
        if self
            .folder_watch
            .as_ref()
            .is_some_and(|watch| watch.folders == folders)
        {
            return;
        }
        self.folder_watch = None;
        if folders.is_empty() {
            return;
        }
        let (sender, receiver) = async_channel::unbounded::<()>();
        let sender_for_poll = sender.clone();
        let watcher = notify::recommended_watcher(move |result: notify::Result<notify::Event>| {
            let Ok(event) = result else {
                return;
            };
            // Reading a file (a thumbnail, the preview, a sound under the
            // mouse) changes nothing in the listing.
            if matches!(event.kind, EventKind::Access(_)) {
                return;
            }
            if sender.try_send(()).is_err() {
                // The pane is gone; its watcher goes with it.
            }
        });
        let mut watcher = match watcher {
            Ok(watcher) => watcher,
            Err(error) => {
                tracing::warn!("cannot watch {}: {error}", self.cwd);
                return;
            }
        };
        // A drive that sends no notifications — the virtual drive of Google
        // Drive for desktop (FAT32 to Windows), and some network drives —
        // accepts the watch and then stays silent, so a file saved there
        // from the cloud only showed after F5. Those folders are looked at
        // every few seconds instead; NTFS and ReFS (local disks, OneDrive)
        // do notify, so they cost nothing.
        let mut polled = Vec::new();
        for folder in &folders {
            match watcher.watch(Path::new(folder), RecursiveMode::NonRecursive) {
                Ok(()) if notifies(Path::new(folder)) => {}
                Ok(()) => polled.push(folder.clone()),
                Err(error) => {
                    tracing::debug!("cannot watch {folder}: {error}");
                    polled.push(folder.clone());
                }
            }
        }
        let poll = (!polled.is_empty()).then(|| poll(polled, sender_for_poll.clone(), cx));
        let task = cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let mut last: Option<Instant> = None;
            while receiver.recv().await.is_ok() {
                cx.background_executor().timer(SETTLE).await;
                if let Some(since) = last.map(|last| last.elapsed())
                    && since < SPACING
                {
                    cx.background_executor().timer(SPACING - since).await;
                }
                while receiver.try_recv().is_ok() {}
                loop {
                    let Ok(done) = this.update(cx, |pane, cx| pane.refresh_from_disk(cx)) else {
                        return;
                    };
                    if done {
                        break;
                    }
                    cx.background_executor().timer(RETRY).await;
                }
                last = Some(Instant::now());
            }
        });
        self.folder_watch = Some(FolderWatch {
            folders,
            _watcher: watcher,
            _task: task,
            _poll: poll,
        });
    }

    /// Reads the folder again because the disk changed. Put off (false)
    /// while it would get in the way: a name being typed, search hits on
    /// screen, a listing already under way.
    fn refresh_from_disk(&mut self, cx: &mut Context<Self>) -> bool {
        if self.renaming.is_some() || self.search_results.is_some() || self.listing_pending {
            return false;
        }
        self.reload(cx);
        cx.notify();
        true
    }
}

// Looks at `folders` every [`POLL`] and signals when what a listing would show
// of them (names, sizes, dates, attributes) changed. The reading runs off the
// UI thread; the first look only records the starting point.
fn poll(
    folders: Vec<String>,
    changed: async_channel::Sender<()>,
    cx: &mut Context<ExplorerPane>,
) -> Task<()> {
    let executor = cx.background_executor().clone();
    cx.background_executor().spawn(async move {
        let mut last: Option<u64> = None;
        loop {
            executor.timer(POLL).await;
            let now = folders.iter().fold(0u64, |sum, folder| {
                sum.wrapping_add(fingerprint(Path::new(folder)))
            });
            if last.is_some_and(|last| last != now) && changed.send(()).await.is_err() {
                return;
            }
            last = Some(now);
        }
    })
}

#[allow(clippy::disallowed_methods)]
fn fingerprint(folder: &Path) -> u64 {
    use std::hash::{Hash, Hasher};
    let Ok(entries) = std::fs::read_dir(folder) else {
        return 0;
    };
    // A sum of per-entry hashes, so the order the drive lists them in does
    // not matter.
    entries.flatten().fold(0u64, |sum, entry| {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        entry.file_name().hash(&mut hasher);
        if let Ok(metadata) = entry.metadata() {
            metadata.len().hash(&mut hasher);
            metadata.modified().ok().hash(&mut hasher);
            #[cfg(windows)]
            std::os::windows::fs::MetadataExt::file_attributes(&metadata).hash(&mut hasher);
        }
        sum.wrapping_add(hasher.finish())
    })
}

// Whether the folder's file system sends change notifications reliably.
#[cfg(windows)]
#[allow(unsafe_code)]
fn notifies(folder: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{GetVolumeInformationW, GetVolumePathNameW};
    use windows::core::PCWSTR;

    let wide: Vec<u16> = folder.as_os_str().encode_wide().chain(Some(0)).collect();
    let mut root = [0u16; 512];
    if unsafe { GetVolumePathNameW(PCWSTR(wide.as_ptr()), &mut root) }.is_err() {
        return true;
    }
    let mut file_system = [0u16; 64];
    if unsafe {
        GetVolumeInformationW(
            PCWSTR(root.as_ptr()),
            None,
            None,
            None,
            None,
            Some(&mut file_system),
        )
    }
    .is_err()
    {
        return true;
    }
    let length = file_system
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(file_system.len());
    let name = String::from_utf16_lossy(&file_system[..length]);
    matches!(name.as_str(), "NTFS" | "ReFS")
}

#[cfg(not(windows))]
fn notifies(_folder: &Path) -> bool {
    true
}
