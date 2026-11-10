use nohrs_core::errors::Result;
use std::ffi::OsStr;
use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

// `FileEntryDto` is a pure data type and lives in `nohrs-models`. It is
// re-exported here so existing `nohrs_services::fs::listing::FileEntryDto`
// references keep resolving without forcing callers through `nohrs-models`.
pub use nohrs_models::file_entry::FileEntryDto;

/// Parameters for a single directory-listing request.
pub struct ListParams<'a> {
    /// Absolute or relative path of the directory to list.
    pub path: &'a str,
    /// Maximum number of entries to return for this page.
    pub limit: usize,
    /// Opaque cursor (a decimal offset) from a previous `next_cursor`, or `None` for the first page.
    pub cursor: Option<&'a str>,
}

/// One page of directory entries plus a cursor for the next page.
pub struct ListResult {
    /// The entries in this page. Sorted case-insensitively by name when the
    /// folder is paged; when it all fits in one answer, in the order the
    /// filesystem lists them, for the caller to sort its own way.
    pub entries: Vec<FileEntryDto>,
    /// Cursor to fetch the following page, or `None` when the directory is exhausted.
    pub next_cursor: Option<String>,
}

/// Lists a directory. The work is synchronous filesystem IO; callers that must
/// keep the UI thread responsive should run it on GPUI's background executor via
/// `cx.background_spawn` (async-runtime.md §2).
#[tracing::instrument(target = "nohrs::op", name = "fs.list_dir", level = "debug", skip_all, fields(path = params.path, limit = params.limit))]
pub fn list_dir_sync(params: ListParams<'_>) -> Result<ListResult> {
    list_dir_impl(params.path, params.limit, params.cursor)
}

fn list_dir_impl(path: &str, limit: usize, cursor: Option<&str>) -> Result<ListResult> {
    let dir = Path::new(path);
    let mut entries: Vec<FileEntryDto> = Vec::new();

    // Everything an entry shows is read here, from what `read_dir` already
    // holds. On Windows `DirEntry::metadata` is the find data
    // `FindNextFileW` returned with the name, and, like `symlink_metadata`,
    // it describes a link rather than its target. Asking `symlink_metadata`
    // by path instead reopened every file. Whole listings, median of 7
    // alternated rounds (`examples/listing_ab.rs`, 04/11/2026, Premiere open):
    // WinSxS\Manifests, 35,628 entries, 2,628 -> 95 ms; System32, 5,218,
    // 174 -> 5.5 ms; a Google Drive folder, 69, 18 -> 0.9 ms. Elsewhere it
    // is the same `lstat` as before.
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let name = os_str_to_string(entry.file_name());
        let mut attributes = 0;
        let (kind, size, modified) = match entry.metadata() {
            Ok(md) => {
                attributes = file_attributes(&md);
                let modified = md
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
                let file_type = md.file_type();
                if file_type.is_dir() {
                    ("dir", 0, modified)
                } else if file_type.is_file() {
                    ("file", md.len(), modified)
                } else if file_type.is_symlink() {
                    // A link made with `mklink /D` or a junction (`mklink /J`)
                    // is shown as what it points at: a folder opens here like
                    // any other, instead of going to Explorer as a file the
                    // default program opens. Only links pay for the second
                    // read. A broken one stays a link.
                    match fs::metadata(entry.path()) {
                        Ok(target) if target.is_dir() => ("dir", 0, modified),
                        Ok(target) => ("file", target.len(), modified),
                        Err(_) => ("symlink", 0, modified),
                    }
                } else {
                    ("other", 0, modified)
                }
            }
            Err(_) => ("unknown", 0, 0),
        };
        entries.push(FileEntryDto {
            path: entry.path().to_string_lossy().into_owned(),
            name,
            kind: kind.to_string(),
            size,
            modified,
            attributes: cloud_attributes(attributes, kind, &entry.path()),
        });
    }
    let total = entries.len();
    let offset = cursor
        .and_then(|s| s.parse::<usize>().ok())
        .unwrap_or(0)
        .min(total);
    // A page needs an order, or the next cursor would not continue where this
    // page stopped. The whole folder in one answer does not: the explorer and
    // the Tree view sort it themselves, by the order the user picked, and this
    // sort was thrown away — 8.7–10.4 ms of 82–101 for WinSxS\Manifests,
    // 35,628 entries (`examples/listing_sort_cost.rs`, 04/11/2026). The key is
    // computed once per entry; `sort_by_key` would lowercase both names again
    // on every comparison.
    if offset > 0 || limit < total {
        entries.sort_by_cached_key(|entry| entry.name.to_lowercase());
    }
    let end = offset.saturating_add(limit).min(total);
    entries.truncate(end);
    entries.drain(..offset);

    let next_cursor = if end < total {
        Some(end.to_string())
    } else {
        None
    };

    Ok(ListResult {
        entries,
        next_cursor,
    })
}

#[cfg(windows)]
fn file_attributes(metadata: &fs::Metadata) -> u32 {
    std::os::windows::fs::MetadataExt::file_attributes(metadata)
}

#[cfg(not(windows))]
fn file_attributes(_metadata: &fs::Metadata) -> u32 {
    0
}

const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
const CLOUD_ATTRIBUTES: u32 = 0x1000 | 0x40000 | 0x80000 | 0x100000 | 0x400000;

// The attributes kept on a row, for its cloud state (OneDrive and the like).
// A reparse point that is not the Cloud Files filter's — a link shown as its
// target, an app alias in WindowsApps, a deduplicated file — is dropped, so
// it does not read as "on this device". Only files that carry no cloud
// attribute of their own pay for the extra look at the tag.
fn cloud_attributes(attributes: u32, kind: &str, path: &Path) -> u32 {
    if attributes & FILE_ATTRIBUTE_REPARSE_POINT == 0 || attributes & CLOUD_ATTRIBUTES != 0 {
        return attributes;
    }
    if kind != "symlink" && is_cloud_placeholder(path) {
        attributes
    } else {
        attributes & !FILE_ATTRIBUTE_REPARSE_POINT
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn is_cloud_placeholder(path: &Path) -> bool {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_ATTRIBUTE_TAG_INFO, FILE_FLAG_BACKUP_SEMANTICS,
        FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        FileAttributeTagInfo, GetFileInformationByHandleEx, OPEN_EXISTING,
    };
    use windows::core::PCWSTR;

    // IO_REPARSE_TAG_CLOUD and its sixteen variants (0x9000001A, 0x9000101A…).
    const CLOUD_TAG: u32 = 0x9000_001A;
    const CLOUD_TAG_MASK: u32 = 0xFFFF_0FFF;

    let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    // No access rights and the reparse point itself: nothing is read, so a
    // placeholder is not downloaded by looking at it.
    let Ok(handle) = (unsafe {
        CreateFileW(
            PCWSTR(wide.as_ptr()),
            0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_OPEN_REPARSE_POINT | FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )
    }) else {
        return false;
    };
    let mut info = FILE_ATTRIBUTE_TAG_INFO::default();
    let read = unsafe {
        GetFileInformationByHandleEx(
            handle,
            FileAttributeTagInfo,
            (&raw mut info).cast(),
            std::mem::size_of::<FILE_ATTRIBUTE_TAG_INFO>() as u32,
        )
    };
    if let Err(error) = unsafe { CloseHandle(handle) } {
        tracing::debug!("closing {}: {error}", path.display());
    }
    read.is_ok() && info.ReparseTag & CLOUD_TAG_MASK == CLOUD_TAG
}

#[cfg(not(windows))]
fn is_cloud_placeholder(_path: &Path) -> bool {
    false
}

fn os_str_to_string(s: impl AsRef<OsStr>) -> String {
    s.as_ref().to_string_lossy().into_owned()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::disallowed_methods)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn list(path: &str, limit: usize, cursor: Option<&str>) -> ListResult {
        list_dir_sync(ListParams {
            path,
            limit,
            cursor,
        })
        .unwrap()
    }

    // A junction (what `mklink /J` makes, and what tools like Steam and
    // OneDrive leave around) lists as the folder it points at, so it opens
    // here; a junction to nothing stays a link.
    #[cfg(windows)]
    #[test]
    fn a_junction_lists_as_the_folder_it_points_at() {
        let dir = tempdir().unwrap();
        let target = dir.path().join("alvo");
        std::fs::create_dir(&target).unwrap();
        std::fs::write(target.join("dentro.txt"), "x").unwrap();
        let junction = |name: &str, to: &std::path::Path| {
            let status = std::process::Command::new("cmd")
                .args(["/C", "mklink", "/J"])
                .arg(dir.path().join(name))
                .arg(to)
                .output()
                .unwrap()
                .status;
            assert!(status.success(), "mklink /J failed");
        };
        junction("ligacao", &target);
        junction("quebrada", &dir.path().join("sumiu"));
        let listed = list(dir.path().to_str().unwrap(), 100, None);
        let kind = |name: &str| {
            listed
                .entries
                .iter()
                .find(|entry| entry.name == name)
                .unwrap()
                .kind
                .clone()
        };
        assert_eq!(kind("ligacao"), "dir");
        assert_eq!(kind("quebrada"), "symlink");
    }

    #[test]
    fn empty_directory_yields_no_entries() {
        let dir = tempdir().unwrap();
        let res = list(&dir.path().to_string_lossy(), 100, None);
        assert!(res.entries.is_empty());
        assert!(res.next_cursor.is_none());
    }

    #[test]
    fn lists_files_and_dirs() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join("Banana"), "yellow").unwrap();
        std::fs::write(dir.path().join("apple"), "x").unwrap();
        std::fs::create_dir(dir.path().join("Cherry")).unwrap();

        // The whole folder in one answer comes in the filesystem's order; the
        // caller sorts it (see `pages_are_sorted_case_insensitively`).
        let res = list(&dir.path().to_string_lossy(), 100, None);
        let mut names: Vec<_> = res.entries.iter().map(|e| e.name.as_str()).collect();
        names.sort_by_key(|name| name.to_lowercase());
        assert_eq!(names, ["apple", "Banana", "Cherry"]);

        let banana = res.entries.iter().find(|e| e.name == "Banana").unwrap();
        assert_eq!(banana.kind, "file");
        assert_eq!(banana.size, 6);
        let cherry = res.entries.iter().find(|e| e.name == "Cherry").unwrap();
        assert_eq!(cherry.kind, "dir");
        assert_eq!(cherry.size, 0);
    }

    #[test]
    fn hidden_files_are_included() {
        let dir = tempdir().unwrap();
        std::fs::write(dir.path().join(".secret"), "x").unwrap();
        let res = list(&dir.path().to_string_lossy(), 100, None);
        assert!(res.entries.iter().any(|e| e.name == ".secret"));
    }

    #[test]
    fn paging_uses_limit_and_cursor() {
        let dir = tempdir().unwrap();
        for n in 0..5 {
            std::fs::write(dir.path().join(format!("f{n}")), "x").unwrap();
        }
        let path = dir.path().to_string_lossy().to_string();

        let first = list(&path, 2, None);
        assert_eq!(first.entries.len(), 2);
        assert_eq!(first.next_cursor.as_deref(), Some("2"));

        let second = list(&path, 2, first.next_cursor.as_deref());
        assert_eq!(second.entries.len(), 2);
        assert_eq!(second.next_cursor.as_deref(), Some("4"));

        // A cursor past the end clamps to the total: an empty final page.
        let beyond = list(&path, 2, Some("99"));
        assert!(beyond.entries.is_empty());
        assert!(beyond.next_cursor.is_none());
    }

    #[test]
    fn pages_are_sorted_case_insensitively() {
        // A cursor only continues where the page before stopped if every page
        // is cut from the same order.
        // NTFS hands names back already in its own case-insensitive order,
        // which compares upper case: there `_` (0x5F) comes after every
        // letter, while the lowercase sort puts it before them (0x5F < 0x61).
        // Without `_` the filesystem's order would pass for the sort.
        let dir = tempdir().unwrap();
        for name in ["delta", "Bravo", "_echo", "alpha", "Charlie"] {
            std::fs::write(dir.path().join(name), "x").unwrap();
        }
        let path = dir.path().to_string_lossy().to_string();
        let mut names = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let page = list(&path, 2, cursor.as_deref());
            names.extend(page.entries.into_iter().map(|entry| entry.name));
            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }
        }
        assert_eq!(names, ["_echo", "alpha", "Bravo", "Charlie", "delta"]);
    }

    #[test]
    fn nonexistent_path_is_an_error() {
        assert!(
            list_dir_sync(ListParams {
                path: "/no/such/dir/here",
                limit: 10,
                cursor: None,
            })
            .is_err()
        );
    }

    #[test]
    fn os_str_to_string_round_trips_utf8() {
        assert_eq!(os_str_to_string("hello.txt"), "hello.txt");
    }
}
