//! What the preview pane lists for a folder (its contents) and for a zip
//! archive (the entries inside it), read off the UI thread.

use nohrs_core::i18n;
use nohrs_core::{k, t, tf};
use std::path::{Path, PathBuf};

use super::entries::SortName;

/// Entries listed before the rest is only counted: a folder of tens of
/// thousands of files would otherwise build a list nobody scrolls through.
const MAX_ITEMS: usize = 2000;

/// One line of a listing preview.
#[derive(Clone, Debug)]
pub(crate) struct ListingItem {
    /// What the line shows: a name, or a path inside an archive.
    pub name: String,
    /// The item on disk, for a folder's own contents; `None` inside a zip.
    pub path: Option<String>,
    /// The archive and member to extract when dragging an archive entry.
    pub archive_member: Option<(std::path::PathBuf, String)>,
    pub is_dir: bool,
    /// Size in bytes (uncompressed, inside a zip); `None` for folders.
    pub size: Option<u64>,
    /// Nesting inside a zip, for indenting; always 0 for a folder.
    pub depth: usize,
    /// Last change, Unix seconds, for a folder's own files (0 inside an
    /// archive): with the size, what tells a waveform is still the file's.
    pub modified: u64,
}

/// The items picked in the preview's listing with Ctrl or Shift, to drag out
/// together. Tied to the listing it was made in (by its address): another
/// folder, or the same one read again, starts with nothing picked.
#[derive(Default)]
pub(crate) struct ListingSelection {
    listing: usize,
    picked: std::collections::BTreeSet<usize>,
    anchor: Option<usize>,
}

impl ListingSelection {
    fn key(listing: &std::rc::Rc<PreviewListing>) -> usize {
        std::rc::Rc::as_ptr(listing) as usize
    }

    // Starts over when the listing is another one.
    fn for_listing(&mut self, listing: &std::rc::Rc<PreviewListing>) -> &mut Self {
        let key = Self::key(listing);
        if self.listing != key {
            *self = Self {
                listing: key,
                ..Self::default()
            };
        }
        self
    }

    /// Whether item `ix` of `listing` is picked.
    pub(crate) fn contains(&self, listing: &std::rc::Rc<PreviewListing>, ix: usize) -> bool {
        self.listing == Self::key(listing) && self.picked.contains(&ix)
    }

    /// Ctrl+click: picks item `ix`, or drops it if it was picked.
    pub(crate) fn toggle(&mut self, listing: &std::rc::Rc<PreviewListing>, ix: usize) {
        let this = self.for_listing(listing);
        if !this.picked.remove(&ix) {
            this.picked.insert(ix);
        }
        this.anchor = Some(ix);
    }

    /// Shift+click: picks everything from the last item clicked to `ix`.
    pub(crate) fn extend_to(&mut self, listing: &std::rc::Rc<PreviewListing>, ix: usize) {
        let this = self.for_listing(listing);
        let from = this.anchor.unwrap_or(ix);
        let (low, high) = if from <= ix { (from, ix) } else { (ix, from) };
        this.picked.extend((low..=high).filter(|ix| {
            listing
                .items
                .get(*ix)
                .is_some_and(|item| item.path.is_some())
        }));
        this.anchor.get_or_insert(ix);
    }

    pub(crate) fn clear(&mut self) {
        *self = Self::default();
    }

    /// The picked items that are on disk, in the listing's order.
    pub(crate) fn paths(&self, listing: &std::rc::Rc<PreviewListing>) -> Vec<std::path::PathBuf> {
        if self.listing != Self::key(listing) {
            return Vec::new();
        }
        self.picked
            .iter()
            .filter_map(|ix| {
                listing
                    .items
                    .get(*ix)?
                    .path
                    .as_deref()
                    .map(std::path::PathBuf::from)
            })
            .collect()
    }
}

/// A folder's or an archive's contents, as the preview pane shows them.
#[derive(Clone, Debug)]
pub(crate) struct PreviewListing {
    pub items: Vec<ListingItem>,
    /// "12 pastas, 40 arquivos" and the like.
    pub summary: String,
    /// Entries left out past [`MAX_ITEMS`].
    pub hidden: usize,
}

/// The contents of `folder`: subfolders first, then files, each by name.
#[allow(clippy::disallowed_methods)]
pub(crate) fn read_folder(folder: &Path, show_hidden: bool) -> std::io::Result<PreviewListing> {
    let mut items: Vec<ListingItem> = std::fs::read_dir(folder)?
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !show_hidden && is_hidden(&entry) {
                return None;
            }
            let metadata = entry.metadata().ok()?;
            Some(ListingItem {
                path: Some(entry.path().to_string_lossy().into_owned()),
                archive_member: None,
                is_dir: metadata.is_dir(),
                size: (!metadata.is_dir()).then_some(metadata.len()),
                depth: 0,
                modified: metadata
                    .modified()
                    .ok()
                    .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                    .map_or(0, |elapsed| elapsed.as_secs()),
                name,
            })
        })
        .collect();
    // Once per name, not per comparison, and the listing's own order: this
    // lowercased both names on every comparison and put `f10` before `f2`,
    // so a folder read in the preview disagreed with the same folder opened
    // (OTIMIZACAO.md §7.5).
    items.sort_by_cached_key(|item| (!item.is_dir, natural_key(&item.name)));
    let folders = items.iter().filter(|item| item.is_dir).count();
    let files = items.len() - folders;
    let summary = if items.is_empty() {
        t!("Pasta vazia").to_string()
    } else {
        format!("{} · {}", count_folders(folders), count_files(files))
    };
    Ok(cap(items, summary))
}

/// The entries of the zip archive at `path`, as a tree: each one under the
/// folder it sits in, indented by depth. Only the archive's directory is
/// read; nothing is decompressed.
#[allow(clippy::disallowed_methods)]
pub(crate) fn read_zip(path: &Path) -> Result<PreviewListing, String> {
    let file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let mut archive =
        zip::ZipArchive::new(std::io::BufReader::new(file)).map_err(|error| error.to_string())?;
    let mut entries = Vec::with_capacity(archive.len());
    for index in 0..archive.len() {
        let Ok(entry) = archive.by_index_raw(index) else {
            continue;
        };
        entries.push((entry.name().to_string(), entry.is_dir(), entry.size()));
    }
    Ok(archive_tree(entries, path))
}

/// Archives the preview lists besides zip: what 7-Zip (or, without it, the
/// `tar.exe` Windows ships) can read the directory of.
pub(crate) fn is_other_archive(path: &str) -> bool {
    let extension = Path::new(path)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    matches!(
        extension.as_str(),
        "rar" | "7z" | "tar" | "tgz" | "gz" | "xz" | "bz2" | "zst" | "cab" | "iso"
    )
}

/// The entries of a .rar, .7z or other archive [`is_other_archive`] names.
///
/// Neither format has a reader worth linking: RAR's only complete one is
/// the C++ unrar, whose licence forbids using it to write RAR. 7-Zip reads
/// both, RAR5 included, is on most machines that have such files, and
/// prints names as UTF-8 when asked to. Without it, `tar.exe` (libarchive,
/// in Windows since 10) reads both too, but writes names in the ANSI code
/// page, so a name outside it comes out with `?` in it.
pub(crate) fn read_archive(path: &Path) -> Result<PreviewListing, String> {
    let entries = match seven_zip() {
        Some(seven) => {
            // `-p` with no password: an encrypted archive fails at once
            // instead of waiting for a password on a console nobody sees.
            let output = hidden_command(&seven)
                .args(["l", "-slt", "-sccUTF-8", "-p", "--"])
                .arg(path)
                .output()
                .map_err(|error| error.to_string())?;
            if !output.status.success() {
                return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
            }
            parse_seven_zip(&String::from_utf8_lossy(&output.stdout))
        }
        None => {
            let output = hidden_command(Path::new(r"C:\Windows\System32\tar.exe"))
                .arg("-tvf")
                .arg(path)
                .output()
                .map_err(|error| error.to_string())?;
            if !output.status.success() {
                return Err(ansi_to_string(&output.stderr).trim().to_string());
            }
            parse_tar_listing(&ansi_to_string(&output.stdout))
        }
    };
    Ok(archive_tree(entries, path))
}

// 7-Zip's console program, where its installer puts it or on the PATH.
fn seven_zip() -> Option<std::path::PathBuf> {
    let installed = ["ProgramFiles", "ProgramW6432", "ProgramFiles(x86)"]
        .iter()
        .filter_map(std::env::var_os)
        .map(|dir| Path::new(&dir).join("7-Zip").join("7z.exe"));
    let on_path = std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths)
                .map(|dir| dir.join("7z.exe"))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    installed
        .chain(on_path)
        .find(|candidate| candidate.is_file())
}

// A console program run without flashing a console window over the app.
fn hidden_command(program: &Path) -> std::process::Command {
    let mut command = std::process::Command::new(program);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command.stdin(std::process::Stdio::null());
    command
}

/// The entries of `7z l -slt`: blocks of `Key = value` lines, one per entry,
/// after a `----------` line that ends the archive's own block.
fn parse_seven_zip(text: &str) -> Vec<(String, bool, u64)> {
    let Some((_, entries)) = text.split_once("\n----------") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut current: Option<(String, bool, u64)> = None;
    for line in entries.lines() {
        let line = line.trim_end_matches('\r');
        let Some((key, value)) = line.split_once(" = ") else {
            continue;
        };
        match (key, current.as_mut()) {
            ("Path", _) => {
                out.extend(current.take());
                current = Some((value.to_string(), false, 0));
            }
            ("Size", Some(entry)) => entry.2 = value.parse().unwrap_or(0),
            ("Folder", Some(entry)) => entry.1 |= value == "+",
            // `D` among a RAR's or a 7z's attributes marks a folder.
            ("Attributes", Some(entry)) => {
                entry.1 |= value
                    .split_whitespace()
                    .next()
                    .is_some_and(|attrs| attrs.contains('D'));
            }
            _ => {}
        }
    }
    out.extend(current);
    out
}

/// The entries of `tar -tvf`, an `ls -l` line each: mode, links, owner,
/// group, size, three fields of date, then the name, which may hold spaces.
fn parse_tar_listing(text: &str) -> Vec<(String, bool, u64)> {
    text.lines()
        .filter_map(|line| {
            let mut rest = line.trim_end_matches('\r');
            let mut fields = Vec::with_capacity(8);
            for _ in 0..8 {
                rest = rest.trim_start();
                let end = rest.find(char::is_whitespace)?;
                fields.push(&rest[..end]);
                rest = &rest[end..];
            }
            let name = rest.strip_prefix(' ').unwrap_or(rest).to_string();
            let is_dir = fields[0].starts_with('d') || name.ends_with('/');
            Some((name, is_dir, fields[4].parse().unwrap_or(0)))
        })
        .collect()
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn ansi_to_string(bytes: &[u8]) -> String {
    use windows::Win32::Globalization::{
        CP_ACP, MULTI_BYTE_TO_WIDE_CHAR_FLAGS, MultiByteToWideChar,
    };
    if bytes.is_empty() {
        return String::new();
    }
    // SAFETY: the first call only measures; the second writes into a buffer
    // of exactly the length the first returned.
    unsafe {
        let len = MultiByteToWideChar(CP_ACP, MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0), bytes, None);
        let mut wide = vec![0u16; len.max(0) as usize];
        MultiByteToWideChar(
            CP_ACP,
            MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0),
            bytes,
            Some(&mut wide),
        );
        String::from_utf16_lossy(&wide)
    }
}

#[cfg(not(windows))]
fn ansi_to_string(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// An archive's entries (`path`, folder?, size) as a tree, folders first at
/// every level and each entry indented under the folder it sits in.
fn archive_tree(entries: Vec<(String, bool, u64)>, archive: &Path) -> PreviewListing {
    let mut paths: std::collections::BTreeMap<String, (bool, Option<u64>)> =
        std::collections::BTreeMap::new();
    let mut total: u64 = 0;
    for (name, is_dir, size) in entries {
        let name = name.replace('\\', "/");
        let trimmed = name.trim_end_matches('/').to_string();
        if trimmed.is_empty() {
            continue;
        }
        // Archives often list only files; their folders are implied.
        let mut parent = String::new();
        for part in trimmed
            .split('/')
            .collect::<Vec<_>>()
            .iter()
            .rev()
            .skip(1)
            .rev()
        {
            if !parent.is_empty() {
                parent.push('/');
            }
            parent.push_str(part);
            paths.entry(parent.clone()).or_insert((true, None));
        }
        if !is_dir {
            total += size;
        }
        paths.insert(trimmed, (is_dir, (!is_dir).then_some(size)));
    }
    // Folders before files at every level, the way the listing orders them.
    let mut keys: Vec<(&String, &(bool, Option<u64>))> = paths.iter().collect();
    keys.sort_by_cached_key(|(path, (is_dir, _))| sort_key(path, *is_dir, &paths));
    let items: Vec<ListingItem> = keys
        .into_iter()
        .map(|(path, (is_dir, size))| ListingItem {
            name: path.rsplit('/').next().unwrap_or(path).to_string(),
            path: None,
            archive_member: Some((archive.to_path_buf(), path.clone())),
            is_dir: *is_dir,
            size: *size,
            depth: path.matches('/').count(),
            modified: 0,
        })
        .collect();
    let folders = items.iter().filter(|item| item.is_dir).count();
    let files = items.len() - folders;
    let summary = tf!(
        "{folders} · {files} · {size} descompactado",
        folders = count_folders(folders),
        files = count_files(files),
        size = human_size(total)
    );
    cap(items, summary)
}

/// Where an archive entry dragged out of the preview lands on disk, made the
/// moment the drag starts. Windows' file drag carries paths, and the Shell
/// builds its data object only from paths that exist, so the file (or folder)
/// is there, empty, before its contents are (`extract_staged`).
///
/// This is how 7-Zip's File Manager drags out of an archive (`PanelDrag.cpp`):
/// the drag starts at once, the extraction runs while the pointer travels,
/// and the drop waits for it to end (`QueryContinueDrag`). Extracting first
/// and dragging after is what failed here: by the time `7z.exe` had written a
/// file the button was usually up, and a drag started without it ends at once.
pub(crate) struct DragStage {
    /// What the drag carries: the file, or the folder with its contents.
    pub(crate) target: PathBuf,
    /// The drag's own temporary folder, removed when the extraction fails.
    directory: PathBuf,
}

pub(crate) fn stage_for_drag(member: &str, is_dir: bool) -> Result<DragStage, String> {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

    let relative = safe_archive_path(member)?;
    let basename = relative
        .file_name()
        .ok_or_else(|| "Nome de arquivo inválido no arquivo compactado".to_string())?
        .to_owned();
    let root = std::env::temp_dir().join("PrimeFiles-ArchiveDrag");
    std::fs::create_dir_all(&root).map_err(|error| error.to_string())?;
    forget_old_drags(&root);
    let directory = loop {
        let nonce = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let candidate = root.join(format!("{}-{nonce}", std::process::id()));
        match std::fs::create_dir(&candidate) {
            Ok(()) => break candidate,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error.to_string()),
        }
    };
    // A folder keeps its path inside the drag's folder, where `7z x` and
    // `tar -x` put it; a file comes out alone, where `7z e` puts it.
    let target = if is_dir {
        directory.join(&relative)
    } else {
        directory.join(basename)
    };
    let made = if is_dir {
        std::fs::create_dir_all(&target)
    } else {
        std::fs::File::create(&target).map(drop)
    };
    if let Err(error) = made {
        let _ = std::fs::remove_dir_all(&directory);
        return Err(error.to_string());
    }
    Ok(DragStage { target, directory })
}

/// Writes the contents of the entry `stage` was made for. Slow (the whole
/// entry is decompressed), so off the UI thread; a failure takes the drag's
/// folder with it.
pub(crate) fn extract_staged(
    stage: &DragStage,
    archive: &Path,
    member: &str,
    is_dir: bool,
) -> Result<(), String> {
    let result = extract_into(stage, archive, member, is_dir);
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&stage.directory);
    }
    result
}

/// [`stage_for_drag`] and [`extract_staged`] in one go.
#[cfg(test)]
pub(crate) fn extract_for_drag(
    archive: &Path,
    member: &str,
    is_dir: bool,
) -> Result<PathBuf, String> {
    let stage = stage_for_drag(member, is_dir)?;
    extract_staged(&stage, archive, member, is_dir)?;
    Ok(stage.target)
}

fn extract_into(
    stage: &DragStage,
    archive: &Path,
    member: &str,
    is_dir: bool,
) -> Result<(), String> {
    if archive
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("zip"))
    {
        match extract_zip(archive, member, is_dir, &stage.target) {
            Ok(()) => return Ok(()),
            // What the built-in reader lacks (LZMA, Deflate64, AES, a split
            // zip) 7-Zip reads, and `tar.exe` reads most of it too.
            Err(error) => {
                tracing::debug!("zip reader could not extract {member}: {error}; trying 7-Zip")
            }
        }
    }
    extract_with_tool(stage, archive, member, is_dir)
}

// Deflate's method number. The zip crate is built without codecs, and then
// names it only by number; the data is inflated here with flate2.
#[allow(deprecated)]
const ZIP_DEFLATE: zip::CompressionMethod = zip::CompressionMethod::Unsupported(8);

#[allow(clippy::disallowed_methods)]
fn extract_zip(archive: &Path, member: &str, is_dir: bool, target: &Path) -> Result<(), String> {
    let file = std::fs::File::open(archive).map_err(|error| error.to_string())?;
    let mut zip =
        zip::ZipArchive::new(std::io::BufReader::new(file)).map_err(|error| error.to_string())?;
    // The names as the preview listed them (`archive_tree`): `/` between
    // parts and no trailing `/` on a folder. Read from the central directory
    // only, so finding an entry costs nothing per entry it skips.
    let name_of = |zip: &zip::ZipArchive<_>, index: usize| {
        zip.name_for_index(index)
            .map(|name| name.replace('\\', "/").trim_end_matches('/').to_string())
    };
    if !is_dir {
        let index = zip
            .index_for_name(member)
            .or_else(|| {
                (0..zip.len()).find(|&index| name_of(&zip, index).as_deref() == Some(member))
            })
            .ok_or_else(|| format!("O item {member} não foi encontrado no ZIP"))?;
        return write_zip_entry(&mut zip, index, target);
    }
    let prefix = format!("{}/", member.trim_end_matches('/'));
    let inside: Vec<(usize, PathBuf)> = (0..zip.len())
        .filter_map(|index| {
            let name = name_of(&zip, index)?;
            let relative = name.strip_prefix(&prefix)?;
            Some(safe_archive_path(relative).map(|relative| (index, relative)))
        })
        .collect::<Result<_, _>>()?;
    for (index, relative) in inside {
        write_zip_entry(&mut zip, index, &target.join(relative))?;
    }
    Ok(())
}

// One zip entry to `out`, stored or deflated, its CRC checked: a truncated or
// damaged entry must not reach the program it is dropped on as a whole file.
fn write_zip_entry<R: std::io::Read + std::io::Seek>(
    zip: &mut zip::ZipArchive<R>,
    index: usize,
    out: &Path,
) -> Result<(), String> {
    use std::io::Write as _;
    let entry = zip.by_index_raw(index).map_err(|error| error.to_string())?;
    if entry.is_dir() {
        return std::fs::create_dir_all(out).map_err(|error| error.to_string());
    }
    if entry.encrypted() {
        return Err("O arquivo compactado tem senha".to_string());
    }
    let (method, crc) = (entry.compression(), entry.crc32());
    let mut reader: Box<dyn std::io::Read + '_> = if method == zip::CompressionMethod::Stored {
        Box::new(entry)
    } else if method == ZIP_DEFLATE {
        Box::new(flate2::read::DeflateDecoder::new(entry))
    } else {
        return Err(format!("compression method {method} not built in"));
    };
    if let Some(parent) = out.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let file = std::fs::File::create(out).map_err(|error| error.to_string())?;
    // A large buffer: written in 8 KiB pieces, a 1 GB video is 130 000 writes.
    let mut writer = std::io::BufWriter::with_capacity(1 << 20, file);
    let mut checked = flate2::CrcReader::new(&mut reader);
    std::io::copy(&mut checked, &mut writer).map_err(|error| error.to_string())?;
    writer.flush().map_err(|error| error.to_string())?;
    if checked.crc().sum() != crc {
        return Err("O arquivo compactado está corrompido".to_string());
    }
    Ok(())
}

// 7-Zip, or without it `tar.exe`, writing straight into the drag's folder.
fn extract_with_tool(
    stage: &DragStage,
    archive: &Path,
    member: &str,
    is_dir: bool,
) -> Result<(), String> {
    if let Some(seven) = seven_zip() {
        // `e` writes the file alone, `x` the folder with its path; `-spd`
        // takes the name literally (a `[` or `*` in it is no wildcard), `-p`
        // with no password fails an encrypted archive instead of waiting on a
        // console nobody sees, and `-aoa` replaces the empty file the drag
        // started with.
        let output = hidden_command(&seven)
            .arg(if is_dir { "x" } else { "e" })
            .args(["-y", "-aoa", "-p", "-spd", "-bsp0", "-sccUTF-8"])
            .arg(format!("-o{}", stage.directory.display()))
            .arg("--")
            .arg(archive)
            .arg(member.replace('/', "\\"))
            .output()
            .map_err(|error| error.to_string())?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(if stderr.is_empty() {
                stdout.trim().to_string()
            } else {
                stderr
            });
        }
        // A name that matched nothing is no failure to 7-Zip.
        if stdout.contains("No files to process") {
            return Err(format!("O item {member} não foi encontrado"));
        }
        return Ok(());
    }
    let tar = Path::new(r"C:\Windows\System32\tar.exe");
    if is_dir {
        let output = hidden_command(tar)
            .arg("-xf")
            .arg(archive)
            .arg("-C")
            .arg(&stage.directory)
            .arg("--")
            .arg(member)
            .output()
            .map_err(|error| error.to_string())?;
        if !output.status.success() {
            return Err(ansi_to_string(&output.stderr).trim().to_string());
        }
        return Ok(());
    }
    // `tar` has no "without its path", so the file comes out on stdout.
    let file = std::fs::File::create(&stage.target).map_err(|error| error.to_string())?;
    let output = hidden_command(tar)
        .arg("-xOf")
        .arg(archive)
        .arg("--")
        .arg(member)
        .stdout(file)
        .stderr(std::process::Stdio::piped())
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(ansi_to_string(&output.stderr).trim().to_string());
    }
    Ok(())
}

// The copies earlier drags left: the program they were dropped on has long
// taken its own copy, so a day later they are only taking up the disk.
fn forget_old_drags(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    let day = std::time::Duration::from_secs(24 * 60 * 60);
    for entry in entries.flatten() {
        let old = entry
            .metadata()
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > day);
        if old && let Err(error) = std::fs::remove_dir_all(entry.path()) {
            tracing::debug!("could not clear {}: {error}", entry.path().display());
        }
    }
}

fn safe_archive_path(member: &str) -> Result<PathBuf, String> {
    let mut path = PathBuf::new();
    for component in member.replace('\\', "/").split('/') {
        if component.is_empty() || component == "." || component == ".." || component.contains(':')
        {
            return Err("Caminho inválido dentro do arquivo compactado".to_string());
        }
        path.push(component);
    }
    if path.as_os_str().is_empty() {
        return Err("Caminho vazio dentro do arquivo compactado".to_string());
    }
    Ok(path)
}

// The key that sorts an archive entry into tree order with folders first at each
// level: every ancestor component tagged as a folder, then the entry itself.
fn sort_key(
    path: &str,
    is_dir: bool,
    paths: &std::collections::BTreeMap<String, (bool, Option<u64>)>,
) -> Vec<(bool, SortName)> {
    let parts: Vec<&str> = path.split('/').collect();
    let mut key = Vec::with_capacity(parts.len());
    let mut prefix = String::new();
    for (index, part) in parts.iter().enumerate() {
        if index > 0 {
            prefix.push('/');
        }
        prefix.push_str(part);
        let dir = if index + 1 == parts.len() {
            is_dir
        } else {
            paths.get(&prefix).is_none_or(|(dir, _)| *dir)
        };
        // `false` sorts first: folders before files.
        key.push((!dir, natural_key(part)));
    }
    key
}

fn cap(mut items: Vec<ListingItem>, summary: String) -> PreviewListing {
    let hidden = items.len().saturating_sub(MAX_ITEMS);
    items.truncate(MAX_ITEMS);
    PreviewListing {
        items,
        summary,
        hidden,
    }
}

// The order the listing sorts names in, Explorer's.
fn natural_key(name: &str) -> SortName {
    SortName::new(name)
}

fn count_folders(n: usize) -> String {
    i18n::plural(n, k!("{n} pasta"), k!("{n} pastas"))
}

fn count_files(n: usize) -> String {
    i18n::plural(n, k!("{n} arquivo"), k!("{n} arquivos"))
}

/// `1.5 MB`, `320 KB`: the size as the listing shows it.
pub(crate) fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

#[cfg(windows)]
fn is_hidden(entry: &std::fs::DirEntry) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    entry
        .metadata()
        .is_ok_and(|metadata| metadata.file_attributes() & FILE_ATTRIBUTE_HIDDEN != 0)
        || entry.file_name().to_string_lossy().starts_with('.')
}

#[cfg(not(windows))]
fn is_hidden(entry: &std::fs::DirEntry) -> bool {
    entry.file_name().to_string_lossy().starts_with('.')
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::disallowed_methods)]
mod tests {
    use super::*;
    use std::io::Write;

    // A zip with only file entries (its folders implied), a name with a
    // space and a nested folder, written stored so no codec is needed.
    fn sample_zip(at: &Path) {
        let file = std::fs::File::create(at).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, body) in [
            ("docs/a b.txt", "alfa"),
            ("docs/sub/c.txt", "gama"),
            ("top.txt", "topo"),
        ] {
            zip.start_file(name, options).unwrap();
            zip.write_all(body.as_bytes()).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn a_file_dragged_out_of_a_zip_is_extracted() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("x.zip");
        sample_zip(&archive);
        let out = extract_for_drag(&archive, "docs/a b.txt", false).unwrap();
        assert_eq!(out.file_name().unwrap(), "a b.txt");
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "alfa");
    }

    // The zips people have are deflated, which the stored sample above is
    // not: dragging out of one failed on the reader built without codecs.
    // `tar -a` (libarchive, in Windows) writes a deflated zip.
    #[cfg(windows)]
    #[test]
    fn a_file_dragged_out_of_a_deflated_zip_is_inflated() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("src");
        std::fs::create_dir_all(source.join("docs")).unwrap();
        let body = "texto que se repete ".repeat(500);
        std::fs::write(source.join("docs").join("a b.txt"), &body).unwrap();
        let archive = dir.path().join("x.zip");
        let status = std::process::Command::new(r"C:\Windows\System32\tar.exe")
            .arg("-a")
            .arg("-cf")
            .arg(&archive)
            .arg("-C")
            .arg(&source)
            .arg("docs")
            .status()
            .unwrap();
        assert!(status.success());
        let file = std::fs::File::open(&archive).unwrap();
        let mut zip = zip::ZipArchive::new(file).unwrap();
        let index = zip.index_for_name("docs/a b.txt").unwrap();
        assert_eq!(
            zip.by_index_raw(index).unwrap().compression(),
            ZIP_DEFLATE,
            "the case under test"
        );
        let out = extract_for_drag(&archive, "docs/a b.txt", false).unwrap();
        assert_eq!(std::fs::read_to_string(&out).unwrap(), body);
        let folder = extract_for_drag(&archive, "docs", true).unwrap();
        assert_eq!(
            std::fs::read_to_string(folder.join("a b.txt")).unwrap(),
            body
        );
    }

    // The drag starts before the extraction: the Shell needs the path to
    // exist then, and the extraction fills in what is already there.
    #[test]
    fn a_drag_out_of_an_archive_has_its_path_before_its_contents() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("x.zip");
        sample_zip(&archive);
        let stage = stage_for_drag("docs/sub/c.txt", false).unwrap();
        assert_eq!(stage.target.file_name().unwrap(), "c.txt");
        assert_eq!(std::fs::metadata(&stage.target).unwrap().len(), 0);
        extract_staged(&stage, &archive, "docs/sub/c.txt", false).unwrap();
        assert_eq!(std::fs::read_to_string(&stage.target).unwrap(), "gama");
    }

    #[test]
    fn a_failed_extraction_leaves_nothing_behind() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("x.zip");
        sample_zip(&archive);
        let stage = stage_for_drag("nao/existe.txt", false).unwrap();
        assert!(extract_staged(&stage, &archive, "nao/existe.txt", false).is_err());
        assert!(!stage.directory.exists());
    }

    #[test]
    fn a_folder_dragged_out_of_a_zip_brings_its_contents() {
        let dir = tempfile::tempdir().unwrap();
        let archive = dir.path().join("x.zip");
        sample_zip(&archive);
        let out = extract_for_drag(&archive, "docs", true).unwrap();
        assert_eq!(out.file_name().unwrap(), "docs");
        assert_eq!(
            std::fs::read_to_string(out.join("a b.txt")).unwrap(),
            "alfa"
        );
        assert_eq!(
            std::fs::read_to_string(out.join("sub").join("c.txt")).unwrap(),
            "gama"
        );
        assert!(!out.join("top.txt").exists(), "only the folder dragged");
    }

    // Through 7-Zip, as .rar/.7z/.iso go; skipped where it is not installed.
    #[cfg(windows)]
    #[test]
    fn a_file_and_a_folder_dragged_out_of_a_7z_are_extracted() {
        let Some(seven) = seven_zip() else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let tree = dir.path().join("tree");
        std::fs::create_dir_all(tree.join("docs").join("sub")).unwrap();
        std::fs::write(tree.join("docs").join("a b.txt"), "alfa").unwrap();
        std::fs::write(tree.join("docs").join("sub").join("c.txt"), "gama").unwrap();
        std::fs::write(tree.join("top.txt"), "topo").unwrap();
        let archive = dir.path().join("x.7z");
        let made = std::process::Command::new(&seven)
            .current_dir(&tree)
            .args(["a", "-y"])
            .arg(&archive)
            .args(["docs", "top.txt"])
            .output()
            .unwrap();
        assert!(
            made.status.success(),
            "{}",
            String::from_utf8_lossy(&made.stdout)
        );
        // The member names as the listing has them: '/' between folders.
        let listing = read_archive(&archive).unwrap();
        let member = |name: &str| {
            listing
                .items
                .iter()
                .find_map(|item| {
                    item.archive_member
                        .as_ref()
                        .filter(|(_, member)| member == name)
                        .map(|(_, m)| m.clone())
                })
                .unwrap_or_else(|| panic!("{name} not listed"))
        };
        let file = extract_for_drag(&archive, &member("docs/a b.txt"), false).unwrap();
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "alfa");
        let folder = extract_for_drag(&archive, &member("docs"), true).unwrap();
        assert_eq!(
            std::fs::read_to_string(folder.join("sub").join("c.txt")).unwrap(),
            "gama"
        );
        assert!(
            !folder.parent().unwrap().join("top.txt").exists(),
            "only the folder dragged"
        );
    }

    #[test]
    fn a_folder_in_the_preview_is_in_the_listing_order() {
        // Numbers as numbers, as the listing (and Explorer) sorts: the
        // preview used to put `f10` before `f2`.
        let dir = tempfile::tempdir().unwrap();
        for name in ["f10.txt", "f2.txt", "F1.txt"] {
            std::fs::write(dir.path().join(name), "x").unwrap();
        }
        let listing = read_folder(dir.path(), false).unwrap();
        let names: Vec<_> = listing
            .items
            .iter()
            .map(|item| item.name.as_str())
            .collect();
        assert_eq!(names, ["F1.txt", "f2.txt", "f10.txt"]);
    }

    #[test]
    fn a_folder_lists_subfolders_first() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.txt"), "a").unwrap();
        std::fs::create_dir(dir.path().join("Zeta")).unwrap();
        std::fs::create_dir(dir.path().join("beta")).unwrap();
        let listing = read_folder(dir.path(), false).unwrap();
        let names: Vec<_> = listing
            .items
            .iter()
            .map(|item| item.name.as_str())
            .collect();
        assert_eq!(names, ["beta", "Zeta", "a.txt"]);
        assert_eq!(listing.summary, "2 pastas · 1 arquivo");
    }

    #[test]
    fn a_zip_lists_its_tree_with_implied_folders() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pack.zip");
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        writer.start_file("readme.txt", options).unwrap();
        writer.write_all(b"hi").unwrap();
        writer.start_file("src/main.rs", options).unwrap();
        writer.write_all(b"fn main() {}").unwrap();
        writer.finish().unwrap();

        let listing = read_zip(&path).unwrap();
        let lines: Vec<_> = listing
            .items
            .iter()
            .map(|item| (item.depth, item.name.as_str(), item.is_dir))
            .collect();
        assert_eq!(
            lines,
            [
                (0, "src", true),
                (1, "main.rs", false),
                (0, "readme.txt", false)
            ]
        );
        assert!(listing.summary.starts_with("1 pasta · 2 arquivos"));
    }

    #[test]
    fn seven_zip_blocks_become_entries() {
        let text = "Listing archive: x.rar\n\n--\nPath = x.rar\nType = Rar5\n\n----------\nPath = docs\\a b.txt\nFolder = -\nSize = 12\nAttributes = A\n\nPath = docs\nFolder = +\nSize = 0\nAttributes = D\n\n";
        let entries = parse_seven_zip(text);
        assert_eq!(
            entries,
            [
                ("docs\\a b.txt".to_string(), false, 12),
                ("docs".to_string(), true, 0)
            ]
        );
        let listing = archive_tree(entries, Path::new("sample.rar"));
        let lines: Vec<_> = listing
            .items
            .iter()
            .map(|item| (item.depth, item.name.as_str()))
            .collect();
        assert_eq!(lines, [(0, "docs"), (1, "a b.txt")]);
    }

    #[test]
    fn tar_lines_keep_spaces_in_names() {
        let text = "-rw-rw-rw-  0 0      0           3 nov  04 11:11 pasta/ação teste.txt\r\ndrwxr-xr-x  0 0      0           0 nov  04 11:11 pasta/\r\n";
        assert_eq!(
            parse_tar_listing(text),
            [
                ("pasta/ação teste.txt".to_string(), false, 3),
                ("pasta/".to_string(), true, 0)
            ]
        );
    }
}

#[cfg(test)]
mod selection_tests {
    use super::{ListingItem, ListingSelection, PreviewListing};
    use std::rc::Rc;

    fn listing(names: &[&str]) -> Rc<PreviewListing> {
        Rc::new(PreviewListing {
            items: names
                .iter()
                .map(|name| ListingItem {
                    name: name.to_string(),
                    path: Some(format!("C:/pasta/{name}")),
                    archive_member: None,
                    is_dir: false,
                    size: Some(1),
                    depth: 0,
                    modified: 0,
                })
                .collect(),
            summary: String::new(),
            hidden: 0,
        })
    }

    #[test]
    fn ctrl_picks_and_drops_shift_picks_a_run() {
        let list = listing(&["a", "b", "c", "d", "e"]);
        let mut picked = ListingSelection::default();
        picked.toggle(&list, 1);
        picked.toggle(&list, 3);
        picked.toggle(&list, 1);
        assert_eq!(
            picked.paths(&list),
            [std::path::PathBuf::from("C:/pasta/d")]
        );
        picked.extend_to(&list, 0);
        let names: Vec<_> = picked
            .paths(&list)
            .iter()
            .map(|path| path.file_name().unwrap().to_owned())
            .collect();
        // From the last item clicked (b, dropped by its Ctrl+click) to a, as
        // in the Explorer; d stays picked.
        assert_eq!(names, ["a", "b", "d"]);
    }

    #[test]
    fn another_listing_starts_with_nothing_picked() {
        let first = listing(&["a", "b"]);
        let second = listing(&["x", "y"]);
        let mut picked = ListingSelection::default();
        picked.toggle(&first, 0);
        assert!(picked.contains(&first, 0));
        assert!(!picked.contains(&second, 0));
        assert!(picked.paths(&second).is_empty());
        picked.toggle(&second, 1);
        assert!(
            !picked.contains(&first, 0),
            "picking in the new listing forgot the old one"
        );
    }
}
