use serde::Serialize;

/// A filesystem entry as produced by the services layer and consumed by the UI.
///
/// Field types are intentionally primitive (`String`, `u64`) so this type stays
/// free of any toolkit or service dependency and can live at the bottom of the
/// dependency graph in `nohrs-models`.
#[derive(Debug, Serialize, Clone)]
pub struct FileEntryDto {
    /// File or directory name (final path component).
    pub name: String,
    /// Full path to the entry.
    pub path: String,
    /// Entry kind as a string (`"file"`, `"dir"`, or `"symlink"`), matching the
    /// values emitted by the listing service.
    pub kind: String,
    /// Size in bytes.
    pub size: u64,
    /// Last modification time as a Unix timestamp in seconds.
    pub modified: u64,
    /// The Windows file attributes the listing read with the name (0 where
    /// there are none, or for rows that are not files on disk). They say
    /// whether a OneDrive-style file is only online or kept on the device.
    pub attributes: u32,
}

/// Where a file of a cloud-synced folder (OneDrive, Dropbox, iCloud…) lives,
/// as the Explorer's Status column shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudState {
    /// Only a placeholder is on the disk; opening downloads it.
    OnlineOnly,
    /// Downloaded, and may be freed again.
    OnDevice,
    /// "Always keep on this device".
    AlwaysKept,
}

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
const FILE_ATTRIBUTE_OFFLINE: u32 = 0x1000;
const FILE_ATTRIBUTE_RECALL_ON_OPEN: u32 = 0x40000;
const FILE_ATTRIBUTE_PINNED: u32 = 0x80000;
const FILE_ATTRIBUTE_UNPINNED: u32 = 0x100000;
const FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS: u32 = 0x400000;

impl FileEntryDto {
    /// The cloud state, or `None` for an ordinary local file. A synced file
    /// that is on the disk carries no attribute of its own but the reparse
    /// point the sync engine (the Cloud Files filter) keeps on it; links are
    /// reparse points too, but the listing already turned those into
    /// `"dir"`/`"symlink"` rows with their own icons, and a link is never
    /// pinned or online-only, so only plain files and folders get here.
    pub fn cloud_state(&self) -> Option<CloudState> {
        cloud_state(self.attributes)
    }
}

/// See [`FileEntryDto::cloud_state`].
pub fn cloud_state(attributes: u32) -> Option<CloudState> {
    if attributes
        & (FILE_ATTRIBUTE_RECALL_ON_DATA_ACCESS
            | FILE_ATTRIBUTE_RECALL_ON_OPEN
            | FILE_ATTRIBUTE_OFFLINE)
        != 0
    {
        Some(CloudState::OnlineOnly)
    } else if attributes & FILE_ATTRIBUTE_PINNED != 0 {
        Some(CloudState::AlwaysKept)
    } else if attributes & (FILE_ATTRIBUTE_REPARSE_POINT | FILE_ATTRIBUTE_UNPINNED) != 0 {
        Some(CloudState::OnDevice)
    } else {
        None
    }
}

/// A filesystem entry with a strongly typed [`FileKind`].
#[derive(Debug, Clone)]
pub struct FileEntry {
    /// File or directory name (final path component).
    pub name: String,
    /// Full path to the entry.
    pub path: String,
    /// Kind of filesystem entry.
    pub kind: FileKind,
    /// Size in bytes.
    pub size: u64,
}

/// The kind of a filesystem entry.
#[derive(Debug, Clone)]
pub enum FileKind {
    /// A regular file.
    File,
    /// A directory.
    Dir,
    /// A symbolic link.
    Link,
}
