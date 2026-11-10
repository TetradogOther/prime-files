//! How full each drive is: the cards above This PC's listing and the small
//! bars beside the drives in the sidebar, as Explorer shows under "Devices
//! and drives".
//!
//! Asking Windows is cheap for a local disk and can hang for seconds on a
//! network drive that went away, so the answers are read on a thread of
//! their own, kept app-wide, and read again at most every [`REFRESH_AFTER`]
//! while something on screen shows them.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use gpui::{App, AppContext as _, Global, Task};
use nohrs_core::telemetry::LogErr;

const REFRESH_AFTER: Duration = Duration::from_secs(5);

/// Space on one drive, in bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Space {
    pub free: u64,
    pub total: u64,
}

impl Space {
    /// Used share, 0..=1.
    pub fn used_fraction(self) -> f32 {
        if self.total == 0 {
            return 0.0;
        }
        (self.total.saturating_sub(self.free) as f64 / self.total as f64) as f32
    }

    /// Explorer turns the bar red past 90% full.
    pub fn nearly_full(self) -> bool {
        self.used_fraction() >= 0.9
    }

    /// "197 GB livres de 931 GB".
    pub fn describe(self) -> String {
        use nohrs_ui::components::file_list::human_bytes;
        nohrs_core::tf!(
            "{free} livres de {total}",
            free = human_bytes(self.free),
            total = human_bytes(self.total)
        )
    }
}

#[derive(Default)]
struct Spaces {
    by_root: HashMap<String, Space>,
    read_at: Option<Instant>,
    reading: bool,
    _task: Option<Task<()>>,
}

impl Global for Spaces {}

/// `C:\` for any spelling of a drive's root (`c:`, `C:\`, `c:/`); `None` for
/// anything that is not one.
#[cfg(target_os = "windows")]
pub(crate) fn drive_root(path: &str) -> Option<String> {
    let mut chars = path.chars();
    let letter = chars.next().filter(char::is_ascii_alphabetic)?;
    if chars.next() != Some(':') {
        return None;
    }
    match chars.as_str() {
        "" | "\\" | "/" => Some(format!("{}:\\", letter.to_ascii_uppercase())),
        _ => None,
    }
}

/// The root of the unit that holds `path`: on Linux, the mounted filesystem
/// it belongs to (`/`, `/home`, a disk mounted on `/mnt/disk`).
#[cfg(not(target_os = "windows"))]
pub(crate) fn drive_root(path: &str) -> Option<String> {
    platform::root_of(path)
}

/// The filesystems mounted on this machine, as the units of the sidebar.
#[cfg(not(target_os = "windows"))]
pub(crate) fn mount_points() -> Vec<std::path::PathBuf> {
    platform::mount_points()
}

/// The filesystems mounted on this machine, with the name of the disk each
/// is on.
#[cfg(not(target_os = "windows"))]
pub(crate) fn mounts() -> Vec<(std::path::PathBuf, String)> {
    platform::mounts()
}

/// The filesystems mounted on this machine, as the units of the sidebar.
#[cfg(target_os = "windows")]
pub(crate) fn mount_points() -> Vec<std::path::PathBuf> {
    platform::read_all()
        .into_keys()
        .map(std::path::PathBuf::from)
        .collect()
}

/// The filesystems mounted on this machine, with the name of the disk each
/// is on.
#[cfg(target_os = "windows")]
pub(crate) fn mounts() -> Vec<(std::path::PathBuf, String)> {
    platform::read_all()
        .into_keys()
        .map(|root| (std::path::PathBuf::from(root), String::new()))
        .collect()
}

/// The space on the drive whose root is `path`, if known; asks Windows again
/// in the background when the last answer is old. The window repaints when
/// the answer arrives.
pub(crate) fn space(path: &str, cx: &mut App) -> Option<Space> {
    let root = drive_root(path)?;
    refresh_if_stale(cx);
    cx.try_global::<Spaces>()?.by_root.get(&root).copied()
}

fn refresh_if_stale(cx: &mut App) {
    let spaces = cx.default_global::<Spaces>();
    let fresh = spaces
        .read_at
        .is_some_and(|at| at.elapsed() < REFRESH_AFTER);
    if fresh || spaces.reading {
        return;
    }
    spaces.reading = true;
    let read = cx.background_spawn(async { platform::read_all() });
    let task = cx.spawn(async move |cx| {
        let by_root = read.await;
        cx.update(|cx| {
            let spaces = cx.global_mut::<Spaces>();
            spaces.reading = false;
            spaces.read_at = Some(Instant::now());
            if spaces.by_root != by_root {
                spaces.by_root = by_root;
                cx.refresh_windows();
            }
        })
        .log_err();
    });
    cx.global_mut::<Spaces>()._task = Some(task);
}

#[cfg(target_os = "windows")]
#[allow(unsafe_code)]
mod platform {
    use super::Space;
    use std::collections::HashMap;
    use windows::Win32::Storage::FileSystem::{GetDiskFreeSpaceExW, GetLogicalDrives};
    use windows::core::HSTRING;

    pub fn read_all() -> HashMap<String, Space> {
        let mask = unsafe { GetLogicalDrives() };
        (0..26u8)
            .filter(|bit| mask & (1 << bit) != 0)
            .filter_map(|bit| {
                let root = format!("{}:\\", char::from(b'A' + bit));
                let mut free = 0u64;
                let mut total = 0u64;
                // The free space this user may fill (quotas count), as
                // Explorer reports it. An empty card reader fails here and is
                // left out.
                unsafe {
                    GetDiskFreeSpaceExW(
                        &HSTRING::from(root.as_str()),
                        Some(&mut free),
                        Some(&mut total),
                        None,
                    )
                }
                .ok()?;
                (total > 0).then_some((root, Space { free, total }))
            })
            .collect()
    }
}

#[cfg(not(target_os = "windows"))]
mod platform {
    //! The units of a Linux machine: the filesystems mounted on it, and how
    //! much space is left on each one.

    use std::collections::HashMap;
    use std::path::{Path, PathBuf};

    use super::Space;

    /// Where the kernel lists what is mounted.
    const MOUNTS: &str = "/proc/mounts";

    /// The filesystems a person browses: the disks and the network shares.
    ///
    /// A Linux machine mounts far more than a person looks at — the pseudo
    /// filesystems the kernel offers (`proc`, `sysfs`, `cgroup`…) are never
    /// in the way. They are left out.
    pub fn mounts() -> Vec<(PathBuf, String)> {
        let Ok(text) = std::fs::read_to_string(MOUNTS) else {
            return vec![(PathBuf::from("/"), String::new())];
        };
        let mut points = Vec::new();
        for line in text.lines() {
            let mut fields = line.split_whitespace();
            let (Some(device), Some(mount)) = (fields.next(), fields.next()) else {
                continue;
            };
            if !is_a_real_filesystem(device) {
                continue;
            }
            let mount = unescape_mount(mount);
            if mount.is_empty() {
                continue;
            }
            let path = PathBuf::from(mount);
            if points.iter().any(|(known, _)| known == &path) {
                continue;
            }
            // The name of the disk, as the device says it: the root of the
            // machine has no name of its own.
            let name = Path::new(device)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| device.to_string());
            points.push((path, name));
        }
        if points.is_empty() {
            points.push((PathBuf::from("/"), String::new()));
        }
        points
    }

    /// The filesystems a person browses, by their path.
    pub fn mount_points() -> Vec<PathBuf> {
        mounts().into_iter().map(|(path, _)| path).collect()
    }

    /// Whether a mount is a disk, a disk partition or a network share, as
    /// opposed to one of the kernel's own pseudo-filesystems.
    fn is_a_real_filesystem(device: &str) -> bool {
        !device.starts_with("proc")
            && !device.starts_with("sysfs")
            && !device.starts_with("devtmpfs")
            && !device.starts_with("devpts")
            && !device.starts_with("tmpfs")
            && !device.starts_with("cgroup")
            && !device.starts_with("cgroup2")
            && !device.starts_with("securityfs")
            && !device.starts_with("debugfs")
            && !device.starts_with("tracefs")
            && !device.starts_with("pstore")
            && !device.starts_with("mqueue")
            && !device.starts_with("hugetlbfs")
            && !device.starts_with("configfs")
            && !device.starts_with("fusectl")
            && !device.starts_with("bpf")
            && !device.starts_with("binfmt_misc")
            && !device.starts_with("autofs")
            && !device.starts_with("efivarfs")
            && !device.starts_with("rpc_pipefs")
            && !device.starts_with("nsfs")
    }

    /// The octal escapes the kernel writes for a space and other characters.
    fn unescape_mount(mount: &str) -> String {
        let mut out = String::with_capacity(mount.len());
        let mut characters = mount.chars();
        while let Some(character) = characters.next() {
            if character != '\\' {
                out.push(character);
                continue;
            }
            let Some(escape) = characters.next() else {
                out.push('\\');
                break;
            };
            match escape {
                '0'..='7' => {
                    let mut code = escape.to_digit(8).unwrap_or(0);
                    for _ in 0..2 {
                        let Some(next) = characters.next() else { break };
                        let Some(digit) = next.to_digit(8) else {
                            out.push(next);
                            break;
                        };
                        code = code * 8 + digit;
                    }
                    out.push(char::from_u32(code).unwrap_or('\u{fffd}'));
                }
                other => out.push(other),
            }
        }
        out
    }

    /// The root of the unit that holds `path`: the mounted filesystem it
    /// belongs to, the deepest one that is above it.
    pub fn root_of(path: &str) -> Option<String> {
        let path = Path::new(path);
        let mut root: Option<PathBuf> = None;
        for mount in mount_points() {
            if !path.starts_with(&mount) {
                continue;
            }
            let deeper = root
                .as_ref()
                .is_none_or(|current| mount.components().count() > current.components().count());
            if deeper {
                root = Some(mount);
            }
        }
        root.map(|root| root.to_string_lossy().into_owned())
    }

    /// How much space is left on every mounted filesystem.
    pub fn read_all() -> HashMap<String, Space> {
        let mut spaces = HashMap::new();
        for mount in mount_points() {
            let Some(space) = space_of(&mount) else {
                continue;
            };
            spaces.insert(mount.to_string_lossy().into_owned(), space);
        }
        spaces
    }

    /// The space of one filesystem: what is free for this user, and what it
    /// holds in all.
    fn space_of(path: &Path) -> Option<Space> {
        let stat = rustix::fs::statvfs(path).ok()?;
        // The size a block takes on this filesystem, and the number of
        // blocks: the total is what the disk has, the free is what this user
        // may still use.
        let block = if stat.f_frsize > 0 {
            stat.f_frsize
        } else {
            stat.f_bsize
        };
        let total = stat.f_blocks.saturating_mul(block);
        let free = stat.f_bavail.saturating_mul(block);
        (total > 0).then_some(Space { free, total })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    fn only_a_bare_drive_is_a_root() {
        assert_eq!(drive_root("c:").as_deref(), Some("C:\\"));
        assert_eq!(drive_root("D:\\").as_deref(), Some("D:\\"));
        assert_eq!(drive_root("C:\\Users"), None);
        assert_eq!(drive_root("\\\\server\\share"), None);
    }

    // On Linux the unit that holds a folder is the filesystem it is on,
    // and every folder of the machine is on the one of its root.
    #[cfg(not(windows))]
    #[test]
    fn the_unit_of_a_folder_is_the_filesystem_it_is_on() {
        assert_eq!(drive_root("/").as_deref(), Some("/"));
        assert_eq!(drive_root("/home").as_deref(), Some("/"));
        assert_eq!(drive_root("/home/user/Documents").as_deref(), Some("/"));
    }

    #[test]
    fn the_bar_turns_red_past_ninety_percent() {
        let space = Space {
            free: 50,
            total: 1000,
        };
        assert!(space.nearly_full());
        assert!((space.used_fraction() - 0.95).abs() < 1e-6);
        assert!(
            !Space {
                free: 500,
                total: 1000
            }
            .nearly_full()
        );
        assert_eq!(Space { free: 0, total: 0 }.used_fraction(), 0.0);
    }
}
