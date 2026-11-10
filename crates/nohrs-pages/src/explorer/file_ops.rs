//! Explorer file operations: delete (trash / permanent), copy / cut / paste with
//! conflict resolution, inline rename, and new folder
//! (`docs/explorer-essentials.md` §1).
//!
//! Every filesystem mutation is routed through `nohrs_services::fs::ops` rather
//! than `std::fs` (§8). Potentially slow work (recursive copy, deleting trees,
//! trashing many items) runs on the background executor; the cheap single-syscall
//! operations (rename, mkdir) run inline for immediate feedback. Errors are
//! surfaced to the footer status bar — the aggregated error dialog and progress
//! UI are tracked separately in #188.

use nohrs_core::i18n;
use nohrs_core::{k, t, tf};
use std::collections::VecDeque;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use gpui::*;
use gpui_component::input::{InputEvent, InputState};
use nohrs_core::errors::{Error, Result};
use nohrs_services::fs::ops::{self, ConflictResolution};

use super::state::ExplorerPane;
use super::types::{PaneEvent, StatusLevel};

/// Whether a clipboard entry should be duplicated (copy) or relocated (cut) when
/// pasted.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClipMode {
    Copy,
    Cut,
}

/// Process-wide explorer clipboard, shared across panes and windows so a copy in
/// one pane can be pasted in another. The paths are mirrored to the system
/// clipboard for interop with external apps (on Windows also as files, for the
/// Explorer and the desktop), but this global stays the source of truth for paste
/// because the system clipboard cannot carry the copy/cut distinction across
/// platforms.
#[derive(Clone, Default)]
pub(crate) struct ExplorerClipboard {
    entry: Option<(ClipMode, Vec<PathBuf>)>,
    // Bumped on every write. A paste records the generation it leaves behind so
    // that, finishing later, it can tell its own clipboard state apart from one
    // the user — or an overlapping paste — has claimed since. Comparing against
    // "is the clipboard empty" cannot: two cut pastes in flight both leave it
    // empty, and whichever finishes first would look like the owner of both.
    generation: u64,
    // Cut pastes whose background move has not finished yet. Emptying `entry` is
    // what stops a second paste from moving the same sources twice, but the
    // paths are also mirrored to the system clipboard, and an empty `entry` is
    // exactly when `read_clipboard` falls back to that text — as a *copy*,
    // racing the move it was meant to stand aside for. While this is non-zero
    // the clipboard is deliberately empty rather than unset, so there is nothing
    // to fall back to.
    cuts_in_flight: u32,
    // The paths of a cut, in lowercase, for the rows to ask about on every
    // frame without walking the list (`is_cut`).
    cut: std::collections::HashSet<String>,
}

impl Global for ExplorerClipboard {}

impl ExplorerClipboard {
    // Replaces the clipboard contents, returning the generation stamped on this
    // write. Mutates in place rather than replacing the global so a write during
    // a cut paste does not lose the in-flight count.
    fn write(entry: Option<(ClipMode, Vec<PathBuf>)>, cx: &mut App) -> u64 {
        let clipboard = cx.default_global::<ExplorerClipboard>();
        clipboard.generation = clipboard.generation.wrapping_add(1);
        clipboard.cut = match &entry {
            Some((ClipMode::Cut, paths)) => paths
                .iter()
                .map(|path| path.to_string_lossy().to_lowercase())
                .collect(),
            _ => Default::default(),
        };
        clipboard.entry = entry;
        let generation = clipboard.generation;
        // Every pane and window draws the cut rows faded.
        cx.refresh_windows();
        generation
    }

    fn generation(cx: &App) -> u64 {
        cx.try_global::<ExplorerClipboard>()
            .map_or(0, |clip| clip.generation)
    }

    fn begin_cut(cx: &mut App) {
        let clipboard = cx.default_global::<ExplorerClipboard>();
        clipboard.cuts_in_flight = clipboard.cuts_in_flight.saturating_add(1);
    }

    fn end_cut(cx: &mut App) {
        let clipboard = cx.default_global::<ExplorerClipboard>();
        clipboard.cuts_in_flight = clipboard.cuts_in_flight.saturating_sub(1);
    }

    fn cut_in_flight(cx: &App) -> bool {
        cx.try_global::<ExplorerClipboard>()
            .is_some_and(|clip| clip.cuts_in_flight > 0)
    }
}

/// Whether `path` was cut and waits for a paste to move it: its icon is drawn
/// faded, as in the Explorer (issue #22). A copy made since, here or in another
/// program, ends it.
pub(crate) fn is_cut(path: &str, cx: &App) -> bool {
    cx.try_global::<ExplorerClipboard>()
        .is_some_and(|clip| !clip.cut.is_empty() && clip.cut.contains(&path.to_lowercase()))
        && explorer_clipboard_is_current()
}

// Puts the sources a cut-paste could not move back on the clipboard, so the user
// can retry them as a move. Without this the cleared clipboard falls through to
// the system clipboard's path text, which carries no cut/copy distinction and so
// retries a failed *move* as a *copy*.
//
// Only restores when the clipboard is still the one this paste left behind:
// anything else means newer state the user can see, which is the live one.
// Takes `&mut App` rather than the pane so a batch outliving the pane that
// started it still restores — the clipboard is process-wide, and the retry
// matters to whatever pane the user is looking at now.
fn retain_failed_cut(failed: Vec<PathBuf>, generation: u64, cx: &mut App) {
    if failed.is_empty() || ExplorerClipboard::generation(cx) != generation {
        return;
    }
    ExplorerClipboard::write(Some((ClipMode::Cut, failed)), cx);
}

// The tests stay on gpui's clipboard so they never replace the real one.
#[cfg(all(windows, not(test)))]
fn write_system_clipboard(mode: ClipMode, paths: &[PathBuf], text: String, cx: &mut App) {
    if let Err(error) = windows_clipboard::write_files(mode, paths, &text) {
        tracing::warn!("could not put files on the Windows clipboard: {error}");
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }
    windows_clipboard::remember_ours();
}

#[cfg(not(all(windows, not(test))))]
fn write_system_clipboard(_mode: ClipMode, _paths: &[PathBuf], text: String, cx: &mut App) {
    cx.write_to_clipboard(ClipboardItem::new_string(text));
}

/// Copies `paths` as Ctrl+C in a listing does, from outside a pane (the
/// search box): pasted here or in the Explorer, the files are copied.
pub(crate) fn copy_files(paths: &[PathBuf], cx: &mut App) {
    if paths.is_empty() {
        return;
    }
    let text = paths
        .iter()
        .map(|path| path.to_string_lossy())
        .collect::<Vec<_>>()
        .join("\n");
    write_system_clipboard(ClipMode::Copy, paths, text, cx);
    ExplorerClipboard::write(Some((ClipMode::Copy, paths.to_vec())), cx);
}

// Whether the explorer's own clipboard is still the newest thing copied: after a
// copy in another program, Paste takes that instead.
#[cfg(all(windows, not(test)))]
fn explorer_clipboard_is_current() -> bool {
    windows_clipboard::still_ours()
}

#[cfg(not(all(windows, not(test))))]
fn explorer_clipboard_is_current() -> bool {
    true
}

#[cfg(all(windows, not(test)))]
fn system_clipboard_files() -> Option<(ClipMode, Vec<PathBuf>)> {
    windows_clipboard::read_files()
}

#[cfg(not(all(windows, not(test))))]
fn system_clipboard_files() -> Option<(ClipMode, Vec<PathBuf>)> {
    None
}

#[cfg(all(windows, not(test)))]
fn forget_system_cut() {
    windows_clipboard::forget_cut();
}

#[cfg(not(all(windows, not(test))))]
fn forget_system_cut() {}

#[cfg(all(windows, not(test)))]
fn adopt_system_clipboard() {
    windows_clipboard::remember_ours();
}

#[cfg(not(all(windows, not(test))))]
fn adopt_system_clipboard() {}

// The Explorer and the desktop paste only CF_HDROP: with path text alone, their
// Paste has nothing to take. gpui's clipboard writes one entry and empties the
// clipboard first, so it cannot carry these formats.
#[cfg(windows)]
#[cfg_attr(test, allow(dead_code))]
#[allow(unsafe_code)]
mod windows_clipboard {
    use std::os::windows::ffi::{OsStrExt, OsStringExt};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardSequenceNumber,
        IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
    };
    use windows::Win32::System::Memory::{
        GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
    };
    use windows::Win32::System::Ole::{CF_HDROP, CF_UNICODETEXT, DROPEFFECT_COPY, DROPEFFECT_MOVE};
    use windows::Win32::UI::Input::KeyboardAndMouse::GetActiveWindow;
    use windows::Win32::UI::Shell::{CFSTR_PREFERREDDROPEFFECT, DROPFILES, DragQueryFileW, HDROP};

    use super::ClipMode;

    // The sequence number the clipboard had right after our last write. While
    // it is unchanged the explorer's own clipboard (which knows copy from cut
    // for sure) is current; once another program writes, that one is stale and
    // the system clipboard is read instead.
    static OURS: AtomicU32 = AtomicU32::new(0);

    pub(super) fn remember_ours() {
        OURS.store(unsafe { GetClipboardSequenceNumber() }, Ordering::Relaxed);
    }

    pub(super) fn still_ours() -> bool {
        OURS.load(Ordering::Relaxed) == unsafe { GetClipboardSequenceNumber() }
    }

    // `EmptyClipboard` with no owner leaves the clipboard ownerless, and the
    // documentation says `SetClipboardData` then fails; the active window of
    // this thread is the explorer window the command came from.
    fn open() -> windows::core::Result<()> {
        let owner = unsafe { GetActiveWindow() };
        unsafe { OpenClipboard((!owner.is_invalid()).then_some(owner)) }
    }

    fn close() {
        if let Err(error) = unsafe { CloseClipboard() } {
            tracing::warn!("could not close the clipboard: {error}");
        }
    }

    pub(super) fn write_files(
        mode: ClipMode,
        paths: &[PathBuf],
        text: &str,
    ) -> windows::core::Result<()> {
        // One open for all the formats: closing between them would let a
        // clipboard listener take the clipboard and fail the next write.
        open()?;
        let result = write_formats(mode, paths, text);
        close();
        result
    }

    /// Empties the system clipboard if it still holds what this app put there.
    /// A cut that was pasted here has moved its files: leaving the cut on the
    /// clipboard would let a later paste (here or in the Explorer) try to move
    /// paths that no longer exist, which is also why the Explorer empties it.
    pub(super) fn forget_cut() {
        if !still_ours() || open().is_err() {
            return;
        }
        if let Err(error) = unsafe { EmptyClipboard() } {
            tracing::warn!("could not empty the clipboard: {error}");
        }
        close();
        remember_ours();
    }

    /// Files the Explorer (or another program) put on the clipboard, with the
    /// effect it asked for: a cut in the Explorer arrives as a move.
    pub(super) fn read_files() -> Option<(ClipMode, Vec<PathBuf>)> {
        let hdrop_format = u32::from(CF_HDROP.0);
        if unsafe { IsClipboardFormatAvailable(hdrop_format) }.is_err() {
            return None;
        }
        open().ok()?;
        let read = read_open_clipboard();
        close();
        read
    }

    fn read_open_clipboard() -> Option<(ClipMode, Vec<PathBuf>)> {
        let handle = unsafe { GetClipboardData(u32::from(CF_HDROP.0)) }.ok()?;
        let drop_handle = HDROP(handle.0);
        let count = unsafe { DragQueryFileW(drop_handle, u32::MAX, None) };
        let paths: Vec<PathBuf> = (0..count)
            .filter_map(|index| {
                let length = unsafe { DragQueryFileW(drop_handle, index, None) } as usize;
                if length == 0 {
                    return None;
                }
                let mut buffer = vec![0u16; length + 1];
                let written = unsafe { DragQueryFileW(drop_handle, index, Some(&mut buffer)) };
                let written = (written as usize).min(length);
                Some(PathBuf::from(std::ffi::OsString::from_wide(
                    &buffer[..written],
                )))
            })
            .collect();
        if paths.is_empty() {
            return None;
        }
        let effect_format = unsafe { RegisterClipboardFormatW(CFSTR_PREFERREDDROPEFFECT) };
        let mode = if effect_format != 0 && preferred_effect(effect_format) & DROPEFFECT_MOVE.0 != 0
        {
            ClipMode::Cut
        } else {
            ClipMode::Copy
        };
        Some((mode, paths))
    }

    fn preferred_effect(format: u32) -> u32 {
        let Ok(handle) = (unsafe { GetClipboardData(format) }) else {
            return 0;
        };
        let global = HGLOBAL(handle.0);
        if unsafe { GlobalSize(global) } < std::mem::size_of::<u32>() {
            return 0;
        }
        let data = unsafe { GlobalLock(global) };
        if data.is_null() {
            return 0;
        }
        let effect = unsafe { data.cast::<u32>().read_unaligned() };
        unsafe { GlobalUnlock(global) }.ok();
        effect
    }

    fn write_formats(mode: ClipMode, paths: &[PathBuf], text: &str) -> windows::core::Result<()> {
        unsafe { EmptyClipboard() }?;

        let text_bytes: Vec<u8> = text
            .encode_utf16()
            .chain(Some(0))
            .flat_map(u16::to_le_bytes)
            .collect();
        set_format(u32::from(CF_UNICODETEXT.0), &text_bytes)?;
        set_format(u32::from(CF_HDROP.0), &drop_files_block(paths))?;

        let effect = match mode {
            ClipMode::Copy => DROPEFFECT_COPY,
            ClipMode::Cut => DROPEFFECT_MOVE,
        };
        let effect_format = unsafe { RegisterClipboardFormatW(CFSTR_PREFERREDDROPEFFECT) };
        if effect_format == 0 {
            return Err(windows::core::Error::from_win32());
        }
        set_format(effect_format, &effect.0.to_le_bytes())
    }

    fn set_format(format: u32, bytes: &[u8]) -> windows::core::Result<()> {
        let global = global_from_bytes(bytes)?;
        if let Err(error) = unsafe { SetClipboardData(format, Some(HANDLE(global.0))) } {
            // The clipboard takes ownership of the memory only on success.
            free_global(global);
            return Err(error);
        }
        Ok(())
    }

    fn global_from_bytes(bytes: &[u8]) -> windows::core::Result<HGLOBAL> {
        let global = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len()) }?;
        let destination = unsafe { GlobalLock(global) };
        if destination.is_null() {
            let error = windows::core::Error::from_win32();
            free_global(global);
            return Err(error);
        }
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), destination.cast::<u8>(), bytes.len());
            // Reports an error when the lock count reaches zero, which is the
            // expected outcome here, so there is no failure to act on.
            GlobalUnlock(global).ok();
        }
        Ok(global)
    }

    fn free_global(global: HGLOBAL) {
        if let Err(error) = unsafe { GlobalFree(Some(global)) } {
            tracing::warn!("could not free clipboard memory: {error}");
        }
    }

    // A DROPFILES header followed by NUL-terminated UTF-16 paths and a final NUL.
    fn drop_files_block(paths: &[PathBuf]) -> Vec<u8> {
        let header_size = std::mem::size_of::<DROPFILES>();
        let mut block = vec![0u8; header_size];
        let files_offset = header_size as u32;
        let wide_characters = 1u32;
        block[..4].copy_from_slice(&files_offset.to_le_bytes());
        block[header_size - 4..].copy_from_slice(&wide_characters.to_le_bytes());
        for path in paths {
            for unit in path.as_os_str().encode_wide().chain(Some(0)) {
                block.extend_from_slice(&unit.to_le_bytes());
            }
        }
        block.extend_from_slice(&0u16.to_le_bytes());
        block
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};

        #[test]
        fn drop_files_block_reads_back_through_the_shell() {
            let paths = [
                PathBuf::from(r"C:\temp\alfa.txt"),
                PathBuf::from(r"C:\pasta com espaço\é 日本語.txt"),
                PathBuf::from(r"\\servidor\pasta\beta"),
            ];
            let global = global_from_bytes(&drop_files_block(&paths)).expect("allocate block");
            let drop_handle = HDROP(global.0);

            let count = unsafe { DragQueryFileW(drop_handle, u32::MAX, None) };
            let read_back: Vec<PathBuf> = (0..count)
                .map(|index| {
                    let length = unsafe { DragQueryFileW(drop_handle, index, None) } as usize;
                    let mut buffer = vec![0u16; length + 1];
                    unsafe { DragQueryFileW(drop_handle, index, Some(&mut buffer)) };
                    PathBuf::from(String::from_utf16_lossy(&buffer[..length]))
                })
                .collect();
            free_global(global);

            assert_eq!(read_back, paths);
        }
    }
}

// The path `name` should take inside `dir`, numbered if the filesystem already
// has that name.
//
// Goes through `ops::destination_in` rather than joining, so a name that is not
// a single plain component — anything a path in the system clipboard, or a path
// dropped on the pane, could carry — cannot aim the paste at a directory the
// user never opened.
//
// An error where the name is taken and cannot be numbered because it is not
// valid UTF-8: a reported failure beats silently writing over whatever is there.
fn free_destination(dir: &Path, name: &OsStr) -> Result<PathBuf> {
    let dst = ops::destination_in(dir, name)?;
    if !ops::would_conflict(&dst) {
        return Ok(dst);
    }
    let name = name.to_str().ok_or_else(|| {
        Error::Other(tf!(
            "{path} já existe e não dá para numerar o nome",
            path = dst.display()
        ))
    })?;
    ops::destination_in(dir, OsStr::new(&ops::unique_name(dir, name)))
}

/// State of an in-progress inline rename of the listing row at `index` (an index
/// into `filtered_entries`).
pub(crate) struct RenameState {
    /// Row currently showing the rename input.
    pub index: usize,
    /// Full path of the entry being renamed.
    pub original_path: PathBuf,
    /// The text field the user edits the name in.
    pub input: Entity<InputState>,
    // Kept alive so the input's `PressEnter` / `Blur` events drive commit; dropped
    // (deregistering the handler) when the rename ends.
    _subscription: Subscription,
}

/// A planned paste whose destination name conflicts are being resolved one at a
/// time through the conflict dialog. Building it only stats the destinations; the
/// actual copy / move runs once every conflict has a resolution.
pub(crate) struct PastePlan {
    mode: ClipMode,
    // Read by the conflict dialog (in the `view` module) to label the prompt.
    pub(crate) dest_dir: PathBuf,
    // Sources with no destination collision: copied / moved as-is.
    clear: Vec<PathBuf>,
    // Conflicting sources still awaiting a user decision (the front is shown).
    pub(crate) pending: VecDeque<PathBuf>,
    // Conflicting sources the user has already decided on.
    resolved: Vec<(PathBuf, ConflictResolution)>,
    // When ticked, the next choice in the dialog applies to every remaining
    // `pending` entry at once (§1.2 "Apply to all").
    pub(crate) apply_to_all: bool,
    // Whether the sources came off the clipboard. Only then does a move
    // consume the clipboard and put back what failed; a drag never touched
    // it and must leave whatever the user copied there alone.
    from_clipboard: bool,
}

// The display name (final path component) of `path`, if it has one.
pub(crate) fn file_name_of(path: &Path) -> Option<String> {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
}

/// Where a rename's initial selection ends: after the name minus its extension,
/// so typing replaces what you meant and `.rs` survives. Directories select
/// whole — their dots are part of the name, not a suffix — as do files with no
/// extension.
///
/// Measured in UTF-16 units because that is what the input's text API takes.
/// Keys off `file_stem` rather than the last `.` so a dotfile stays whole:
/// `.gitignore` is all stem, where `rfind('.')` would select nothing at all.
fn rename_selection_end(name: &str, is_dir: bool) -> usize {
    let whole = name.encode_utf16().count();
    if is_dir {
        return whole;
    }
    Path::new(name)
        .file_stem()
        .map(|stem| stem.to_string_lossy().encode_utf16().count())
        .filter(|end| *end > 0)
        .unwrap_or(whole)
}

// Performs a single copy or move from `src` to `dst` according to `mode`.
//
// Claims `dst` rather than writing over it. Every destination here was chosen by
// looking at the filesystem — `free_destination`, `unique_name`, the conflict
// dialog — and between that look and this call the name can be taken by anything
// else on the machine. A replacing write would destroy what took it; refusing
// costs one reported failure.
fn apply_one(mode: ClipMode, src: &Path, dst: &Path) -> ops::ClaimResult<()> {
    match mode {
        ClipMode::Copy => ops::copy_path_no_replace(src, dst),
        ClipMode::Cut => ops::move_path_no_replace(src, dst).map(|_| ()),
    }
}

/// What an operation that did happen still has to say for itself.
///
/// The return type of the paste helpers, which `Result<()>` could not express.
/// `Ok(())` and `Err(_)` are "it worked" and "it did not", and a cross-volume
/// cut whose copy landed and whose removal of the source gave up partway is
/// neither: the entry is under the name the user asked for, and part of the
/// source is still where it was.
#[derive(Debug, PartialEq, Eq)]
#[must_use]
enum Applied {
    /// Nothing left over. The only thing the batch counts.
    Cleanly,
    /// The whole entry is at the destination, and clearing up after the move did
    /// not finish. One entry per thing left behind.
    WithLeftover(Vec<Leftover>),
}

/// One thing an operation finished and could not clear away.
///
/// Two fields because the footer has one line and two jobs to do with it. With a
/// single leftover it can afford to say what happened; with several it can only
/// say where they are — and where is the half the user can act on, so that is
/// the half that has to survive being summarised.
#[derive(Debug, PartialEq, Eq)]
struct Leftover {
    /// The whole sentence, naming both paths. What the site logged, and what the
    /// footer shows when this is the only one.
    message: String,
    /// Where the leftover actually sits: the path the user would have to open.
    at: PathBuf,
}

impl Applied {
    fn leftover(message: String, at: PathBuf) -> Self {
        Self::WithLeftover(vec![Leftover { message, at }])
    }

    // Adds a leftover to whatever this already carries. One overwrite can strand
    // two things — the staging copy it built and the backup of the entry it
    // replaced — and the user needs both places, not whichever came first.
    fn and(self, message: String, at: PathBuf) -> Self {
        let next = Leftover { message, at };
        match self {
            Self::Cleanly => Self::WithLeftover(vec![next]),
            Self::WithLeftover(mut all) => {
                all.push(next);
                Self::WithLeftover(all)
            }
        }
    }
}

/// What a batch of operations has to report when it is done.
///
/// Two lists rather than one, because the footer has to tell three things apart
/// and `errors.is_empty()` only ever told it two. A leftover is not a failure:
/// counting it as one puts the source back on the clipboard, and the retry then
/// numbers around the name this very batch just took.
#[derive(Default)]
pub(crate) struct OpReport {
    /// One per operation that did not happen. These decide the footer.
    failures: Vec<String>,
    /// One per thing an operation finished and could not clear away.
    leftovers: Vec<Leftover>,
}

impl OpReport {
    pub(crate) fn failed(&mut self, message: String) {
        self.failures.push(message);
    }

    #[cfg(test)]
    pub(crate) fn failures(&self) -> &[String] {
        &self.failures
    }

    // Records whichever of the two an [`Applied`] turned out to be.
    fn applied(&mut self, applied: Applied) {
        if let Applied::WithLeftover(left) = applied {
            self.leftovers.extend(left);
        }
    }

    // The one line the footer gets, and what it is claiming.
    //
    // A failure outranks a leftover: the leftover message is about work that is
    // done, and saying so while something else in the same batch did not happen
    // would read as though the batch went fine. The failures are logged here
    // because the footer only has room for their count.
    fn footer(&self, total: usize, success_label: &str) -> (StatusLevel, String) {
        if !self.failures.is_empty() {
            for failure in &self.failures {
                tracing::error!("explorer file operation failed: {failure}");
            }
            return (
                StatusLevel::Error,
                tf!(
                    "{failed} de {total} falharam",
                    failed = self.failures.len(),
                    total = total
                ),
            );
        }
        match self.leftovers.as_slice() {
            [] => (StatusLevel::Info, success_label.to_string()),
            // One of them fits whole, and it is the whole point of the level.
            // Joined with a dash rather than "but", which the message itself
            // already carries — "3 item(s) moved, but /a was copied to /b, but
            // the original could not be removed".
            [only] => (
                StatusLevel::Warning,
                format!("{success_label} — {}", only.message),
            ),
            // Several do not, so the sentences go and the places stay. Sending
            // the user to the log instead would be the thing this level exists
            // to stop: a leftover nobody can find is what the `warn!` alone
            // already was.
            //
            // The count is of places, and says so: one overwrite can strand two
            // of them, so counting it against `success_label`'s items would read
            // as "1 item(s) pasted — 2 left something behind".
            many => (
                StatusLevel::Warning,
                tf!(
                    "{done} — sobrou algo em {count} lugares: {places}",
                    done = success_label,
                    count = many.len(),
                    places = many
                        .iter()
                        .map(|left| left.at.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", "),
                ),
            ),
        }
    }
}

// [`apply_one`] to a name nothing else is waiting on, clearing away its own
// half-written work if it cannot finish.
//
// A partial left at a fresh name is not a rollback problem — nothing is waiting
// to come back to it — but it is still the wrong thing to leave. It appears in
// the listing as an ordinary folder with no sign that it is incomplete, and it
// takes the name: `free_destination` numbers around it, so the retry lands at
// `report (2)` and the wreckage keeps `report` for good.
//
// Removing it costs only the time already spent, never data:
// `Leftover::PartialDestination` is the one case that comes with the source
// intact, and `discard_partial_destination` refuses anything else — a name this
// call never took, or a whole copy whose source deletion is what failed.
//
// That last one is not reported as a failure at all. A cross-volume cut whose
// copy landed and whose removal of the source then gave up partway has put the
// whole entry under the name the user asked for, so this returns
// `Applied::WithLeftover` rather than an error. Reporting it as a failure would
// hand the source back to the retry, and the retry would number around the name
// this very call just took — the same `report (2)` as above, this time beside a
// finished `report`.
fn apply_to_free_name(mode: ClipMode, src: &Path, dst: &Path) -> Result<Applied> {
    let Err(failure) = apply_one(mode, src, dst) else {
        return Ok(Applied::Cleanly);
    };
    if failure.leftover() == ops::Leftover::WholeDestination {
        let message = tf!(
            "{source} foi copiado para {destination}, mas o original não pôde ser removido por inteiro: {failure}",
            source = src.display(),
            destination = dst.display(),
            failure = failure,
        );
        // Logged here because a leftover is the one outcome `footer` does not
        // write out: it shows the message once and then drops it, so without
        // this line there would be no durable record. The strandings in
        // `overwrite_apply` need no such line — they become failures, and
        // `footer` logs every one of those itself.
        //
        // `warn!` rather than `error!` because what is left sits at `src`, in
        // the listing, rather than at a hidden scratch path.
        tracing::warn!("{message}");
        return Ok(Applied::leftover(message, src.to_path_buf()));
    }
    drop_claimed(&failure, dst);
    Err(failure.into())
}

// Overwrites `dst` with `src` without risking data loss on failure: the existing
// destination is moved aside first, and only deleted once the paste succeeds;
// if the paste fails, the original is rolled back into place. Staging aside (vs.
// deleting upfront) also makes a directory a clean replace rather than a merge.
//
// The new entry is built at a path only this call knows and moved onto `dst` at
// the end, so the destination is claimed once the replacement is whole rather
// than held open for the length of a copy.
fn overwrite_apply(mode: ClipMode, src: &Path, dst: &Path) -> Result<Applied> {
    if !ops::would_conflict(dst) {
        return apply_to_free_name(mode, src, dst);
    }
    let backup = scratch_path(dst, "old");
    if let Err(failure) = ops::move_path_no_replace(dst, &backup) {
        drop_claimed(&failure, &backup);
        // The same stranding the staging step can suffer, one step earlier: the
        // copy to `backup` landed and only clearing `dst` gave up partway, so
        // the whole original is at a hidden scratch path and `dst` holds what
        // the removal spared.
        //
        // This one stays an error, and does not go to `Applied::WithLeftover`.
        // The overwrite did not happen: what the user asked for is not at `dst`,
        // and a retry is the right next step. What the leftover channel is for
        // is the opposite case, where retrying would redo finished work. But the
        // error has to carry the scratch path, because otherwise the batch
        // reports "1 of 1 failed" and the only record of where the original went
        // is a log line.
        if failure.leftover() == ops::Leftover::WholeDestination {
            let message = tf!(
                "{destination} não pôde ser esvaziado depois de copiado à parte; o original inteiro está em {backup}",
                destination = dst.display(),
                backup = backup.display(),
            );
            return Err(Error::Other(format!("{failure} — {message}")));
        }
        return Err(failure.into());
    }
    let staged = scratch_path(dst, "new");
    let error = match build_and_commit(mode, src, dst, &staged) {
        Ok(applied) => {
            let Err(error) = ops::delete_permanent(&backup) else {
                return Ok(applied);
            };
            // The overwrite is done — `dst` holds the replacement — and what is
            // left is the entry it replaced, at a scratch path with no route to
            // it from the listing. Litter rather than loss, since replacing it
            // is what the user asked for, but litter that is their own data, so
            // it is named rather than logged and forgotten.
            // Opening on the scratch path rather than on `{dst} was
            // replaced, but`, which is how the staging message opens: the
            // two can be joined, and repeating the clause made the footer
            // say the same thing twice before getting to either path.
            let message = tf!(
                "a cópia do original guardada em {backup} não pôde ser removida depois de substituir {destination}: {error}",
                backup = backup.display(),
                destination = dst.display(),
                error = error,
            );
            tracing::warn!("{message}");
            return Ok(applied.and(message, backup.clone()));
        }
        Err(error) => error,
    };
    // Put the original back. Whatever took `dst` in the meantime is not ours to
    // replace, so a failure here leaves the original at `backup` and says where.
    if let Err(failure) = ops::move_path_no_replace(&backup, dst) {
        // Unless the rollback itself got as far as taking `dst`: then what is
        // there is the rollback's own fragment, and it has to go for the message
        // below — the original is at `backup` — to be the whole truth.
        drop_claimed(&failure, dst);
        let message = tf!(
            "não foi possível restaurar {destination} depois de uma substituição que falhou (original guardado em {backup}): {failure}",
            destination = dst.display(),
            backup = backup.display(),
            failure = failure,
        );
        // The overwrite's own error is what went wrong first, but this is the
        // one the user has to act on: their original is not at `dst` and not in
        // the listing at all. Reported together rather than in place of, so the
        // cause is not lost either.
        return Err(Error::Other(format!("{error} — {message}")));
    }
    Err(error)
}

// Builds the replacement at `staged` and takes `dst` with it, leaving nothing of
// its own behind when it cannot. `dst` is free on entry — the caller moved what
// was there to the backup.
fn build_and_commit(mode: ClipMode, src: &Path, dst: &Path, staged: &Path) -> Result<Applied> {
    if let Err(failure) = apply_one(mode, src, staged) {
        // Anything a *partial* failure left at `staged` is a fragment the source
        // outlived: a rename that fails moves nothing, and the copy-and-delete
        // fallback deletes the source only once the copy is whole. That is what
        // makes it provable rather than a guess from whether `src` happens to be
        // occupied now, when something else may have re-created it since — and
        // what the failure did not write there is not ours to remove.
        drop_claimed(&failure, staged);
        // One failure does take the source away, and it is the reason the
        // sentence above says "partial": a cut whose copy to `staged` landed and
        // whose removal of `src` then gave up partway. `staged` holds the whole
        // original and `src` holds whatever `remove_dir_all` spared, so the one
        // thing that must not happen is the whole copy going quietly — it is at
        // a hidden scratch path nobody would think to look at.
        //
        // Still an error rather than a leftover, for the reason the backup step
        // gives: the overwrite did not happen, the caller is about to roll `dst`
        // back, and a retry is what the user wants. The scratch path rides along
        // on the error so the batch reports it instead of only the log.
        if failure.leftover() == ops::Leftover::WholeDestination {
            let message = tf!(
                "{source} não pôde ser esvaziado depois de copiado à parte; a cópia inteira está em {copy}",
                source = src.display(),
                copy = staged.display(),
            );
            return Err(Error::Other(format!("{failure} — {message}")));
        }
        return Err(failure.into());
    }
    // Past here `apply_one` returned `Ok`, which for a cut is what says the
    // source is gone and `staged` holds the only copy of it there is.
    if let Err(failure) = ops::move_path_no_replace(staged, dst) {
        // Where the filesystem has no no-replace rename this was a copy, and one
        // that failed partway left a partial `dst` that would block the rollback
        // the caller is about to run. Removing it is safe only because the
        // failure says the name was this call's: `AlreadyExists` cannot, since
        // the kernel reports a collision *at* `dst` and one below it alike, and
        // reading the first as the second deletes a stranger's directory.
        drop_claimed(&failure, dst);
        // `WholeDestination` is not a failure to undo, or to report as one. The
        // replacement reached `dst` intact — which is the whole of what the user
        // asked for — and only clearing `staged` afterwards gave up partway.
        // Rolling back from here would take a finished overwrite apart, and a
        // cut would put whatever `remove_dir_all` spared where the user expects
        // their file, next to the whole copy. What is left is litter at a
        // scratch path, so the operation stands and says where the litter is.
        if failure.leftover() == ops::Leftover::WholeDestination {
            let message = tf!(
                "{destination} foi substituído, mas a cópia temporária em {copy} não pôde ser apagada: {failure}",
                destination = dst.display(),
                copy = staged.display(),
                failure = failure,
            );
            tracing::warn!("{message}");
            return Ok(Applied::leftover(message, staged.to_path_buf()));
        }
        match mode {
            ClipMode::Cut => restore_staged(src, staged),
            ClipMode::Copy => drop_staged(staged),
        }
        return Err(failure.into());
    }
    Ok(Applied::Cleanly)
}

// Puts a cut's source back where it came from. `staged` holds the only copy of
// it, so a failure here leaves it sitting there and says where, rather than
// deleting it.
fn restore_staged(src: &Path, staged: &Path) {
    if !ops::would_conflict(staged) {
        return;
    }
    if let Err(failure) = ops::move_path_no_replace(staged, src) {
        // A restore that got as far as taking the source's name and then failed
        // left a fragment standing where the source was; the whole copy is still
        // at `staged`, and the message below only tells the truth once that
        // fragment is gone.
        drop_claimed(&failure, src);
        tracing::error!(
            "failed to put {} back after a move that could not finish (it is at {}): {failure}",
            src.display(),
            staged.display(),
        );
    }
}

// Removes a staged entry the source outlived.
fn drop_staged(staged: &Path) {
    use nohrs_core::telemetry::LogErr as _;
    if ops::would_conflict(staged) {
        ops::delete_permanent(staged).log_err();
    }
}

// Clears away what a failed no-replace write left at the path it was writing to
// — and only when that is the write's own half-written work.
//
// The decision comes from the operation rather than from what happens to occupy
// the path now, because the cases are indistinguishable afterwards and cost very
// different things. A name the write never took holds someone else's entry; a
// whole copy whose source deletion failed may be the only copy left, since
// `remove_dir_all` gives up partway through. Removing either cannot be undone.
fn drop_claimed(failure: &ops::ClaimFailure, dst: &Path) {
    use nohrs_core::telemetry::LogErr as _;
    failure.discard_partial_destination(dst).log_err();
}

// A sibling path of `dst` that does not yet exist, for one side of an overwrite
// to work at. A sibling so the final rename stays within one filesystem, and
// hidden and suffixed so it is recognizable if a crash leaves one behind.
fn scratch_path(dst: &Path, role: &str) -> PathBuf {
    let parent = dst.parent().unwrap_or_else(|| Path::new("."));
    let name = file_name_of(dst).unwrap_or_default();
    parent.join(ops::unique_name(parent, &format!(".{name}.nohrs-{role}")))
}

impl ExplorerPane {
    // Runs `op` (which performs the filesystem work and returns one message per
    // failure) on the background executor, then reloads the listing and reports
    // the outcome in the status bar. `total` and `success_label` describe the
    // batch for the success / partial-failure messages.
    pub(crate) fn run_fs_op<F>(
        &mut self,
        total: usize,
        success_label: String,
        cx: &mut Context<Self>,
        op: F,
    ) where
        F: FnOnce() -> OpReport + Send + 'static,
    {
        self.run_fs_op_with(
            total,
            success_label,
            cx,
            move || (op(), ()),
            |(), _| Vec::new(),
        );
    }

    // [`run_fs_op`](Self::run_fs_op) for batches that carry an outcome of their
    // own beyond the failure messages. `on_complete` takes `&mut App` rather
    // than the pane, and runs whether or not the pane outlived the batch, so an
    // outcome owning process-wide state is not dropped along with the pane that
    // started the work. It returns the paths the batch put somewhere: the pane
    // selects the ones in its folder and asks for their pictures again.
    pub(crate) fn run_fs_op_with<T, F, G>(
        &mut self,
        total: usize,
        success_label: String,
        cx: &mut Context<Self>,
        op: F,
        on_complete: G,
    ) where
        T: Send + 'static,
        F: FnOnce() -> (OpReport, T) + Send + 'static,
        G: FnOnce(T, &mut App) -> Vec<PathBuf> + 'static,
    {
        use nohrs_core::telemetry::LogErr as _;
        let task = cx.background_spawn(async move { op() });
        // A copy of big files takes a while: the status bar says how far it
        // is, and the copied files' rows too (they repaint with it).
        let done = std::rc::Rc::new(std::cell::Cell::new(false));
        cx.spawn({
            let done = done.clone();
            async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                loop {
                    cx.background_executor()
                        .timer(std::time::Duration::from_millis(300))
                        .await;
                    if done.get() {
                        return;
                    }
                    let Some((count, copied, total)) =
                        nohrs_services::fs::ops::transfers::summary()
                    else {
                        continue;
                    };
                    let percent = if total > 0 {
                        copied.saturating_mul(100) / total
                    } else {
                        0
                    };
                    let text = tf!(
                        "Copiando {count} arquivo(s)… {percent}% ({copied} de {total})",
                        count = count,
                        percent = percent,
                        copied = super::preview_listing::human_size(copied),
                        total = super::preview_listing::human_size(total),
                    );
                    if this
                        .update(cx, |pane, cx| {
                            pane.set_status(StatusLevel::Info, text);
                            cx.notify();
                        })
                        .is_err()
                    {
                        return;
                    }
                }
            }
        })
        .detach();
        cx.spawn(move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut cx = cx.clone();
            async move {
                let (report, outcome) = task.await;
                done.set(true);
                let landed = cx
                    .update(|cx| on_complete(outcome, cx))
                    .log_err()
                    .unwrap_or_default();
                // Before the update, not inside it: this is what writes the
                // failures to the log, and a batch can outlive the pane that
                // started it. Computed in there, a closed pane would take the
                // whole record of what went wrong with it.
                let (level, text) = report.footer(total, &success_label);
                if let Err(error) = this.update(&mut cx, |pane, cx| {
                    pane.reload(cx);
                    pane.landed(&landed, cx);
                    pane.set_status(level, text);
                    cx.notify();
                }) {
                    // Not a failure: the work happened and is logged. There is
                    // just no longer a status bar to put the outcome in.
                    tracing::debug!("the pane a file operation was started from is gone: {error}");
                }
            }
        })
        .detach();
    }

    // ---- Delete (§1.1) ----

    /// Moves the selected entries to the OS trash.
    pub(crate) fn trash_selection(&mut self, cx: &mut Context<Self>) {
        let paths = self.selected_paths();
        if paths.is_empty() {
            return;
        }
        // The same ledger `noh rm` writes, on the platforms that need one: macOS
        // keeps its trash index inside Finder's private `.DS_Store` and exposes
        // nothing to read it back, so an item trashed without a record here can
        // never be restored — not by `noh trash restore`, not by us. Where that
        // record cannot be written, the delete is refused instead: putting the
        // item in the trash would succeed, and it would be a one-way door.
        let ledger = self.trash_ledger.clone();
        if let Err(error) = ledger.to_record() {
            self.set_status(
                StatusLevel::Error,
                tf!("Não dá para mandar para a Lixeira: {error}", error = error),
            );
            cx.notify();
            return;
        }
        let total = paths.len();
        let label = i18n::plural(
            total,
            k!("{n} item enviado para a Lixeira"),
            k!("{n} itens enviados para a Lixeira"),
        );
        self.run_fs_op_with(
            total,
            label,
            cx,
            move || {
                let mut report = OpReport::default();
                // Read in here rather than captured: `to_record` borrows from
                // `ledger`, which this closure owns.
                let record = match ledger.to_record() {
                    Ok(record) => record,
                    Err(error) => {
                        for path in &paths {
                            report.failed(format!("{path}: {error}"));
                        }
                        return (report, Vec::new());
                    }
                };
                // One call for the whole selection: on Windows that is one Shell
                // operation instead of one per item (`ops::trash_paths`).
                let paths: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
                let failed = ops::trash_paths(&paths, record);
                for (path, error) in &failed {
                    report.failed(format!("{}: {error}", path.display()));
                }
                let trashed: Vec<PathBuf> = paths
                    .into_iter()
                    .filter(|path| !failed.iter().any(|(gone, _)| gone == path))
                    .collect();
                (report, trashed)
            },
            |trashed, _| {
                remember(Undo::Trashed(trashed));
                Vec::new()
            },
        );
    }

    // What a batch put somewhere, back on the UI thread after the reload.
    //
    // Selected when it is in this folder, as the Explorer does after a paste or
    // a drop (issue #21): in a folder of thousands the new file was left for
    // the user to find. And its pictures are asked for again: a folder being
    // pasted into lists the file while it is still arriving, the Shell has no
    // thumbnail for half a file, and the slot then waited for its next retry
    // with the type's icon in its place (issue #29).
    pub(crate) fn landed(&mut self, paths: &[PathBuf], cx: &mut Context<Self>) {
        let here: Vec<String> = paths
            .iter()
            .filter(|path| {
                path.parent()
                    .is_some_and(|parent| super::same_path(&parent.to_string_lossy(), &self.cwd))
            })
            .map(|path| path.to_string_lossy().into_owned())
            .collect();
        for path in &here {
            self.thumbnails.forget(path, cx);
        }
        let Some(first) = here.first().cloned() else {
            return;
        };
        self.select_after_listing = here;
        // Selects `first` now or once the reload lands; the others join it
        // there (`take_select_after_listing`).
        self.reveal(&first);
        if !self.listing_pending {
            self.take_select_after_listing();
        }
    }

    /// Adds the rest of a batch to the selection `reveal` just made of its
    /// first item.
    pub(crate) fn take_select_after_listing(&mut self) {
        let paths = std::mem::take(&mut self.select_after_listing);
        if paths.len() < 2 {
            return;
        }
        for path in &paths[1..] {
            if let Some(entry) = self
                .filtered_entries
                .iter()
                .find(|entry| super::same_path(&entry.path, path))
            {
                self.selection.insert(entry.path.clone());
            }
        }
        // A batch is not one file to look at.
        self.preview_selection_pending = false;
    }

    /// Ctrl+Z: takes back the last batch — a deletion (issue #8), a move or a
    /// drop (issue #28), a copy (issue #33), a rename or a new folder. What is
    /// taken back goes on the redo list, for Ctrl+Y. Batches from every tab
    /// share one history, newest first, as Explorer's do.
    pub(crate) fn undo_trash(&mut self, cx: &mut Context<Self>) {
        match with_history(|history| history.undo.pop()).flatten() {
            Some(batch) => self.replay(batch, true, cx),
            None => {
                self.set_status(StatusLevel::Info, t!("Nada para desfazer").to_string());
                cx.notify();
            }
        }
    }

    /// Ctrl+Y (or Ctrl+Shift+Z): does again what Ctrl+Z took back. A new
    /// operation empties the list, as in any editor: redoing over it would
    /// act on files that are no longer where the batch left them.
    pub(crate) fn redo(&mut self, cx: &mut Context<Self>) {
        match with_history(|history| history.redo.pop()).flatten() {
            Some(batch) => self.replay(batch, false, cx),
            None => {
                self.set_status(StatusLevel::Info, t!("Nada para refazer").to_string());
                cx.notify();
            }
        }
    }

    // Plays a batch backwards (`back`, Ctrl+Z) or forwards again (Ctrl+Y).
    // Each kind has two moves that are each other's opposite: a deletion is
    // undone by restoring and redone by deleting; a copy or a new folder is
    // undone by sending it to the Recycle Bin, from where redoing brings it
    // back; a move or a rename goes one way or the other. Only what worked
    // goes on the other list.
    fn replay(&mut self, batch: Undo, back: bool, cx: &mut Context<Self>) {
        let kind = batch.kind();
        match batch {
            Undo::Moved(pairs) | Undo::Renamed(pairs) => self.replay_moves(kind, pairs, back, cx),
            Undo::Trashed(paths) if back => self.replay_restore(kind, paths, back, cx),
            Undo::Trashed(paths) => self.replay_trash(kind, paths, back, cx),
            Undo::Copied(paths) | Undo::Created(paths) if back => {
                self.replay_trash(kind, paths, back, cx)
            }
            Undo::Copied(paths) | Undo::Created(paths) => {
                self.replay_restore(kind, paths, back, cx)
            }
        }
    }

    // Brings items back from the Recycle Bin.
    fn replay_restore(
        &mut self,
        kind: UndoKind,
        paths: Vec<PathBuf>,
        back: bool,
        cx: &mut Context<Self>,
    ) {
        let total = paths.len();
        let label = i18n::plural(
            total,
            k!("{n} item restaurado"),
            k!("{n} itens restaurados"),
        );
        self.run_fs_op_with(
            total,
            label,
            cx,
            move || {
                // On a thread of its own: the `trash` crate initializes COM as a
                // single-threaded apartment and panics on a pool thread already
                // in the multithreaded one, and a panic on gpui's pool aborts
                // the app (as for deleting, `ops::os_trash_delete`).
                let restored = std::thread::Builder::new()
                    .name("trash-restore".into())
                    .spawn(move || restore_batch(paths));
                match restored.map(|thread| thread.join()) {
                    Ok(Ok(outcome)) => outcome,
                    Ok(Err(_)) | Err(_) => {
                        let mut report = OpReport::default();
                        report.failed(t!("A restauração falhou").to_string());
                        (report, Vec::new())
                    }
                }
            },
            move |restored, _| {
                file_replayed(Undo::items(kind, restored.clone()), back);
                restored
            },
        );
    }

    // Sends items to the Recycle Bin: a deletion done again, or a copy or a
    // new folder taken back (from there a wrong undo can still be taken back).
    fn replay_trash(
        &mut self,
        kind: UndoKind,
        paths: Vec<PathBuf>,
        back: bool,
        cx: &mut Context<Self>,
    ) {
        let ledger = self.trash_ledger.clone();
        if let Err(error) = ledger.to_record() {
            self.set_status(
                StatusLevel::Error,
                tf!("Não dá para mandar para a Lixeira: {error}", error = error),
            );
            cx.notify();
            return;
        }
        let total = paths.len();
        let label = if kind == UndoKind::Copied {
            i18n::plural(
                total,
                k!("{n} cópia enviada para a Lixeira"),
                k!("{n} cópias enviadas para a Lixeira"),
            )
        } else {
            i18n::plural(
                total,
                k!("{n} item enviado para a Lixeira"),
                k!("{n} itens enviados para a Lixeira"),
            )
        };
        self.run_fs_op_with(
            total,
            label,
            cx,
            move || {
                let mut report = OpReport::default();
                let record = match ledger.to_record() {
                    Ok(record) => record,
                    Err(error) => {
                        report.failed(error.to_string());
                        return (report, Vec::new());
                    }
                };
                let (present, gone): (Vec<PathBuf>, Vec<PathBuf>) = paths
                    .into_iter()
                    .partition(|path| std::fs::symlink_metadata(path).is_ok());
                for path in &gone {
                    report.failed(tf!(
                        "{path}: não está mais onde estava",
                        path = path.display()
                    ));
                }
                let failed = ops::trash_paths(&present, record);
                for (path, error) in &failed {
                    report.failed(format!("{}: {error}", path.display()));
                }
                let trashed: Vec<PathBuf> = present
                    .into_iter()
                    .filter(|path| !failed.iter().any(|(gone, _)| gone == path))
                    .collect();
                (report, trashed)
            },
            move |trashed, _| {
                file_replayed(Undo::items(kind, trashed), back);
                Vec::new()
            },
        );
    }

    // Moves each item of a move or rename back where it came from (`back`),
    // or to where it went again. A name taken there since is left alone, as
    // the Explorer does, and said in the footer.
    fn replay_moves(
        &mut self,
        kind: UndoKind,
        pairs: Vec<(PathBuf, PathBuf)>,
        back: bool,
        cx: &mut Context<Self>,
    ) {
        let total = pairs.len();
        let label = match (kind, back) {
            (UndoKind::Renamed, true) => {
                i18n::plural(total, k!("{n} nome desfeito"), k!("{n} nomes desfeitos"))
            }
            (UndoKind::Renamed, false) => {
                i18n::plural(total, k!("{n} item renomeado"), k!("{n} itens renomeados"))
            }
            (_, true) => i18n::plural(
                total,
                k!("{n} item movido de volta"),
                k!("{n} itens movidos de volta"),
            ),
            (_, false) => i18n::plural(total, k!("{n} item movido"), k!("{n} itens movidos")),
        };
        self.run_fs_op_with(
            total,
            label,
            cx,
            move || {
                let mut report = OpReport::default();
                let mut done = Vec::new();
                // Undone newest first, so a folder moved after its parent is
                // undone before the parent it went into; redone in order.
                let ordered: Vec<(PathBuf, PathBuf)> = if back {
                    pairs.into_iter().rev().collect()
                } else {
                    pairs
                };
                for (from, to) in ordered {
                    let (src, dst) = if back { (&to, &from) } else { (&from, &to) };
                    if !ops::would_conflict(src) {
                        report.failed(tf!(
                            "{path}: não está mais onde estava",
                            path = src.display()
                        ));
                        continue;
                    }
                    // A rename that only changed capitals (`foto.jpg` to
                    // `Foto.jpg`) finds its own source at the destination.
                    if !same_place(src, dst) && ops::would_conflict(dst) {
                        report.failed(tf!(
                            "{path}: o lugar de destino já está ocupado",
                            path = dst.display()
                        ));
                        continue;
                    }
                    match apply_to_free_name(ClipMode::Cut, src, dst) {
                        Ok(applied) => {
                            report.applied(applied);
                            done.push((from, to));
                        }
                        Err(error) => report.failed(format!("{}: {error}", src.display())),
                    }
                }
                if back {
                    done.reverse();
                }
                (report, done)
            },
            move |done, cx| {
                let landed: Vec<PathBuf> = done
                    .iter()
                    .map(|(from, to)| if back { from.clone() } else { to.clone() })
                    .collect();
                // Tags follow the item, as they do on a rename.
                for (from, to) in &done {
                    let (old, new) = if back { (to, from) } else { (from, to) };
                    super::tags::Tags::moved(&old.to_string_lossy(), &new.to_string_lossy(), cx);
                }
                file_replayed(Undo::pairs(kind, done), back);
                landed
            },
        );
    }

    /// Whether Ctrl+Z has something to undo.
    pub(crate) fn can_undo_trash(&self) -> bool {
        with_history(|history| !history.undo.is_empty()).unwrap_or(false)
    }

    /// Whether Ctrl+Y has something to redo.
    pub(crate) fn can_redo(&self) -> bool {
        with_history(|history| !history.redo.is_empty()).unwrap_or(false)
    }

    /// What Ctrl+Z would take back, for the menu to name it.
    pub(crate) fn undo_kind(&self) -> Option<UndoKind> {
        with_history(|history| history.undo.last().map(Undo::kind)).flatten()
    }

    /// Whether what Ctrl+Z would undo is a move (the menu names it).
    #[cfg(test)]
    pub(crate) fn undo_is_move(&self) -> bool {
        self.undo_kind() == Some(UndoKind::Moved)
    }

    /// Whether what Ctrl+Z would undo is a copy (the menu names it).
    #[cfg(test)]
    pub(crate) fn undo_is_copy(&self) -> bool {
        self.undo_kind() == Some(UndoKind::Copied)
    }

    pub(crate) fn delete_permanent_paths(&mut self, paths: Vec<String>, cx: &mut Context<Self>) {
        let total = paths.len();
        let label = i18n::plural(total, k!("{n} item excluído"), k!("{n} itens excluídos"));
        self.run_fs_op(total, label, cx, move || {
            let mut report = OpReport::default();
            for path in paths {
                if let Err(error) = ops::delete_permanent(Path::new(&path)) {
                    report.failed(format!("{path}: {error}"));
                }
            }
            report
        });
    }

    // ---- Copy / cut / paste (§1.2) ----

    /// Marks the selection for copy on the next paste.
    pub(crate) fn copy_selection(&mut self, cx: &mut Context<Self>) {
        self.set_clipboard(ClipMode::Copy, cx);
    }

    /// Marks the selection for move on the next paste.
    pub(crate) fn cut_selection(&mut self, cx: &mut Context<Self>) {
        self.set_clipboard(ClipMode::Cut, cx);
    }

    fn set_clipboard(&mut self, mode: ClipMode, cx: &mut Context<Self>) {
        let paths = self.selected_paths();
        if paths.is_empty() {
            return;
        }
        let buffers: Vec<PathBuf> = paths.iter().map(PathBuf::from).collect();
        write_system_clipboard(mode, &buffers, paths.join("\n"), cx);
        ExplorerClipboard::write(Some((mode, buffers)), cx);
        let message = match mode {
            ClipMode::Copy => i18n::plural(
                paths.len(),
                k!("{n} item copiado"),
                k!("{n} itens copiados"),
            ),
            ClipMode::Cut => i18n::plural(
                paths.len(),
                k!("{n} item recortado"),
                k!("{n} itens recortados"),
            ),
        };
        self.set_status(StatusLevel::Info, message);
        cx.notify();
    }

    /// Whether Paste has anything to paste.
    pub(crate) fn can_paste(&self, cx: &App) -> bool {
        self.read_clipboard(cx).is_some()
    }

    fn read_clipboard(&self, cx: &App) -> Option<(ClipMode, Vec<PathBuf>)> {
        let current = explorer_clipboard_is_current();
        if current
            && let Some((mode, paths)) = cx
                .try_global::<ExplorerClipboard>()
                .and_then(|clip| clip.entry.clone())
        {
            return Some((mode, paths));
        }
        if current && ExplorerClipboard::cut_in_flight(cx) {
            // Emptied by a cut paste that is still moving those very sources.
            // Falling back now would hand them straight back as a copy.
            return None;
        }
        if let Some(files) = system_clipboard_files() {
            return Some(files);
        }
        // Fall back to the system clipboard as newline-separated path text (e.g.
        // a path copied from elsewhere), treated as a copy.
        let text = cx.read_from_clipboard()?.text()?;
        let paths: Vec<PathBuf> = text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(PathBuf::from)
            .collect();
        (!paths.is_empty()).then_some((ClipMode::Copy, paths))
    }

    /// Pastes the clipboard contents into the current directory, prompting for
    /// each name collision (§1.2).
    pub(crate) fn paste_into_cwd(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let has_pending = self.prepare_paste(cx);
        if self.paste_plan.is_none() {
            return;
        }
        if has_pending {
            self.open_conflict_dialog(window, cx);
        } else {
            self.execute_paste_plan(cx);
        }
    }

    // Builds the paste plan from the clipboard into `self.paste_plan`, splitting
    // sources into collision-free and conflicting sets without touching the UI.
    // Returns whether any conflict needs the dialog. Separated from
    // `paste_into_cwd` so it can be exercised without opening a dialog.
    pub(crate) fn prepare_paste(&mut self, cx: &mut Context<Self>) -> bool {
        self.paste_plan = None;
        let Some((mode, sources)) = self.read_clipboard(cx) else {
            return false;
        };
        if !explorer_clipboard_is_current() {
            // Copied in another program: adopt it, so a cut from the Explorer
            // is consumed and retried exactly like one made here.
            ExplorerClipboard::write(Some((mode, sources.clone())), cx);
            adopt_system_clipboard();
        }
        let dest_dir = PathBuf::from(&self.cwd);
        self.prepare_transfer(mode, sources, dest_dir, true)
    }

    // The plan for copying or moving `sources` into `dest_dir`, in
    // `self.paste_plan`; returns whether a conflict needs the dialog. Shared by
    // paste and by files dropped on the window.
    pub(crate) fn prepare_transfer(
        &mut self,
        mode: ClipMode,
        sources: Vec<PathBuf>,
        dest_dir: PathBuf,
        from_clipboard: bool,
    ) -> bool {
        self.paste_plan = None;
        let mut clear = Vec::new();
        let mut pending = VecDeque::new();
        let mut resolved = Vec::new();
        // Destination names already claimed by an earlier source in this same
        // batch, so two sources sharing a basename (possible via the system
        // clipboard) don't silently overwrite each other. A name is claimed
        // whether or not the destination is already occupied: a conflicting name
        // resolved as Overwrite is still one destination, so a second source
        // aiming at it has to be numbered too.
        let mut claimed = std::collections::HashSet::new();
        for src in sources {
            let Some(name) = src.file_name() else {
                continue;
            };
            let dst = dest_dir.join(name);
            if dst == src {
                // Pasting an item back into its own directory: a copy becomes a
                // numbered duplicate; a cut onto itself is a no-op.
                if mode == ClipMode::Copy {
                    resolved.push((src, ConflictResolution::Rename));
                }
                continue;
            }
            let occupied = ops::would_conflict(&dst);
            if !claimed.insert(dst) {
                // Another source already targets this name; number it instead of
                // letting the later paste clobber the earlier one.
                resolved.push((src, ConflictResolution::Rename));
            } else if occupied {
                pending.push_back(src);
            } else {
                clear.push(src);
            }
        }
        let has_pending = !pending.is_empty();
        self.paste_plan = Some(PastePlan {
            mode,
            dest_dir,
            clear,
            pending,
            resolved,
            apply_to_all: false,
            from_clipboard,
        });
        has_pending
    }

    /// Records the user's choice for the conflict currently shown, advancing to
    /// the next one. Returns `true` when no conflicts remain to resolve.
    pub(crate) fn resolve_current_conflict(
        &mut self,
        choice: ConflictResolution,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(plan) = self.paste_plan.as_mut() else {
            return true;
        };
        if plan.apply_to_all {
            while let Some(src) = plan.pending.pop_front() {
                plan.resolved.push((src, choice));
            }
        } else if let Some(src) = plan.pending.pop_front() {
            plan.resolved.push((src, choice));
        }
        let done = plan.pending.is_empty();
        cx.notify();
        done
    }

    /// Toggles the conflict dialog's "apply to all" checkbox.
    pub(crate) fn set_apply_to_all(&mut self, value: bool, cx: &mut Context<Self>) {
        if let Some(plan) = self.paste_plan.as_mut() {
            plan.apply_to_all = value;
            cx.notify();
        }
    }

    /// Abandons an in-progress paste (the conflict dialog's Cancel).
    pub(crate) fn cancel_paste(&mut self, cx: &mut Context<Self>) {
        if self.paste_plan.take().is_some() {
            cx.notify();
        }
    }

    /// Executes the resolved paste plan on the background executor.
    pub(crate) fn execute_paste_plan(&mut self, cx: &mut Context<Self>) {
        let Some(plan) = self.paste_plan.take() else {
            return;
        };
        let PastePlan {
            mode,
            dest_dir,
            clear,
            resolved,
            from_clipboard,
            ..
        } = plan;
        // Skips do nothing, so drop them before counting — otherwise the success
        // message would claim more items were pasted than actually were.
        let resolved: Vec<(PathBuf, ConflictResolution)> = resolved
            .into_iter()
            .filter(|(_, resolution)| *resolution != ConflictResolution::Skip)
            .collect();
        let total = clear.len() + resolved.len();
        if total == 0 {
            return;
        }
        // A cut consumes the clipboard: clear it now so a second paste, fired
        // while this one is still running, does not try to move the sources a
        // second time. Whatever fails to move is put back afterwards by
        // `retain_failed_cut`, which uses this generation to recognize the
        // clipboard state it left behind.
        let cut_generation = (mode == ClipMode::Cut && from_clipboard).then(|| {
            forget_system_cut();
            ExplorerClipboard::begin_cut(cx);
            ExplorerClipboard::write(None, cx)
        });
        let label = if mode == ClipMode::Cut {
            i18n::plural(total, k!("{n} item movido"), k!("{n} itens movidos"))
        } else {
            i18n::plural(total, k!("{n} item colado"), k!("{n} itens colados"))
        };
        self.run_fs_op_with(
            total,
            label,
            cx,
            move || {
                let mut report = OpReport::default();
                let mut failed = Vec::new();
                // Where each item went, and whether moving it back undoes it
                // (not for one that replaced an item already there).
                let mut landed: Vec<(PathBuf, PathBuf, bool)> = Vec::new();
                for src in clear {
                    // Owned so the borrow of `src` ends before `src` is moved
                    // into `failed`.
                    let Some(name) = src.file_name().map(OsStr::to_os_string) else {
                        continue;
                    };
                    // The plan reserved this name by comparing paths lexically,
                    // which is not how the filesystem compares them: on a
                    // case-insensitive volume `foo.txt` and `FOO.txt` are one
                    // destination, and the second source here would replace the
                    // first. Asking the filesystem instead also covers a name
                    // that appeared after the plan was built.
                    let dst = match free_destination(&dest_dir, &name) {
                        Ok(dst) => dst,
                        Err(error) => {
                            report.failed(format!("{}: {error}", src.display()));
                            failed.push(src);
                            continue;
                        }
                    };
                    match apply_to_free_name(mode, &src, &dst) {
                        Ok(applied) => {
                            report.applied(applied);
                            landed.push((src, dst, true));
                        }
                        Err(error) => {
                            report.failed(format!("{}: {error}", src.display()));
                            failed.push(src);
                        }
                    }
                }
                for (src, resolution) in resolved {
                    let Some(name) = src.file_name().map(OsStr::to_os_string) else {
                        continue;
                    };
                    let result = match resolution {
                        // Filtered out above; kept for exhaustiveness without panicking.
                        ConflictResolution::Skip => continue,
                        ConflictResolution::Rename => {
                            free_destination(&dest_dir, &name).and_then(|dst| {
                                apply_to_free_name(mode, &src, &dst)
                                    .map(|applied| (applied, dst, true))
                            })
                        }
                        ConflictResolution::Overwrite => ops::destination_in(&dest_dir, &name)
                            .and_then(|dst| {
                                overwrite_apply(mode, &src, &dst)
                                    .map(|applied| (applied, dst, false))
                            }),
                    };
                    match result {
                        Ok((applied, dst, undoable)) => {
                            report.applied(applied);
                            landed.push((src, dst, undoable));
                        }
                        Err(error) => {
                            report.failed(format!("{}: {error}", src.display()));
                            failed.push(src);
                        }
                    }
                }
                (report, (failed, landed))
            },
            move |(failed, landed), cx| {
                if let Some(generation) = cut_generation {
                    retain_failed_cut(failed, generation, cx);
                    ExplorerClipboard::end_cut(cx);
                }
                // An item that replaced one already there is left out of both:
                // taking it back would not bring the replaced one back.
                match mode {
                    ClipMode::Cut => remember(Undo::Moved(
                        landed
                            .iter()
                            .filter(|(_, _, undoable)| *undoable)
                            .map(|(from, to, _)| (from.clone(), to.clone()))
                            .collect(),
                    )),
                    ClipMode::Copy => remember(Undo::Copied(
                        landed
                            .iter()
                            .filter(|(_, _, undoable)| *undoable)
                            .map(|(_, to, _)| to.clone())
                            .collect(),
                    )),
                }
                landed.into_iter().map(|(_, to, _)| to).collect()
            },
        );
    }

    // ---- Rename + new folder (§1, §6) ----

    /// Starts an inline rename of the row at `index`, focusing a text field
    /// seeded with the current name.
    pub(crate) fn begin_rename(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self.filtered_entries.get(index) else {
            return;
        };
        let original_path = PathBuf::from(&entry.path);
        let name = entry.name.clone();
        let selection_end = rename_selection_end(&name, entry.kind == "dir");
        let input = cx.new(|cx| InputState::new(window, cx));
        input.update(cx, |state, cx| {
            // Seed the field *and* pre-select the part being renamed in one
            // step. `set_value` always parks the caret at the end, and the
            // selected range itself is not public, so this IME entry point is
            // the only way in: inserting into the empty field lets it place the
            // selection, and `unmark_text` then drops the composition marker so
            // the result renders as ordinary selected text.
            state.replace_and_mark_text_in_range(
                Some(0..0),
                &name,
                Some(0..selection_end),
                window,
                cx,
            );
            state.unmark_text(window, cx);
            state.focus(window, cx);
        });
        let subscription = cx.subscribe_in(
            &input,
            window,
            |this, _input, event: &InputEvent, window, cx| {
                match event {
                    // Commit on Enter, and on Blur so clicking away also saves
                    // (Finder-style). Enter tears the field down first, so the
                    // trailing Blur finds no active rename and is a no-op.
                    InputEvent::PressEnter { .. } | InputEvent::Blur => {
                        this.commit_rename(window, cx)
                    }
                    _ => {}
                }
            },
        );
        self.renaming = Some(RenameState {
            index,
            original_path,
            input,
            _subscription: subscription,
        });
        cx.notify();
    }

    /// Abandons an in-progress rename without touching the filesystem, returning
    /// focus to the listing. Bound to Escape while the field is open, and used
    /// when the row being renamed goes away underneath the field — navigating to
    /// another directory, say — since the field is positioned by row index and
    /// would otherwise re-attach to whichever entry now occupies that slot.
    pub(crate) fn cancel_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.discard_rename(cx) {
            cx.focus_self(window);
        }
    }

    /// [`cancel_rename`](Self::cancel_rename) for callers without a `Window`
    /// (pane-sync navigation). Returns whether a rename was in progress. Focus
    /// is left alone, so the field's own blur handling settles it.
    pub(crate) fn discard_rename(&mut self, cx: &mut Context<Self>) -> bool {
        if self.renaming.take().is_some() {
            cx.notify();
            return true;
        }
        false
    }

    /// Commits the in-progress rename, resolving a name collision by numbering
    /// (§1.2 "Rename"). An empty or unchanged name cancels.
    pub(crate) fn commit_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        use nohrs_core::telemetry::LogErr as _;
        let Some(state) = self.renaming.take() else {
            return;
        };
        // The rename field had keyboard focus; return it to the listing so the
        // user can keep navigating and acting on entries without re-clicking.
        cx.focus_self(window);
        let new_name = state.input.read(cx).value().trim().to_string();
        let original_path = state.original_path;
        let original_name = file_name_of(&original_path).unwrap_or_default();
        if new_name.is_empty() || new_name == original_name {
            cx.notify();
            return;
        }
        // Everything from here asks the disk — whether the name is taken, which
        // number is free, the rename itself — and in a folder on a share that
        // stopped answering any of those would freeze the window (OTIMIZACAO.md
        // §2.12). Until 04/11/2026 they ran on the UI thread. The field is
        // already closed; the row keeps its old name until the answer.
        let task = cx.background_spawn(async move {
            // Typed by the user, so it can be anything. A name that is not a
            // single plain component is left as it is, for `rename_in_place` to
            // reject with the one message for it: numbering `../notes.txt`
            // would quietly turn a rename that was going to fail into one that
            // renames the entry to a sibling of whatever `..` reached. And a
            // name taken only by the entry itself (`foto.jpg` to `Foto.jpg`)
            // is not taken: numbering it was a defect.
            let final_name = match original_path.parent() {
                Some(parent)
                    if ops::destination_in(parent, OsStr::new(&new_name))
                        .is_ok_and(|dst| ops::would_collide_with_another(&original_path, &dst)) =>
                {
                    ops::unique_name(parent, &new_name)
                }
                _ => new_name,
            };
            let renamed = ops::rename_in_place(&original_path, &final_name);
            (original_path, final_name, renamed)
        });
        cx.spawn(async move |this: WeakEntity<Self>, cx| {
            let (original_path, final_name, renamed) = task.await;
            this.update(cx, |pane, cx| {
                pane.finish_rename(&original_path, &final_name, renamed, cx)
            })
            .log_err();
        })
        .detach();
        cx.notify();
    }

    // The rename's answer, back on the UI thread.
    fn finish_rename(
        &mut self,
        original_path: &Path,
        final_name: &str,
        renamed: Result<PathBuf>,
        cx: &mut Context<Self>,
    ) {
        match renamed {
            Ok(renamed) => {
                // The selection is kept by path, so without this the refresh
                // drops the row just renamed from it; Explorer keeps it
                // selected.
                let (old, new) = (
                    original_path.to_string_lossy().into_owned(),
                    renamed.to_string_lossy().into_owned(),
                );
                if self.selection.remove(&old) {
                    self.selection.insert(new.clone());
                }
                for held in [&mut self.selection_anchor, &mut self.active_path] {
                    if held.as_deref() == Some(old.as_str()) {
                        *held = Some(new.clone());
                    }
                }
                // Its tags go with it.
                super::tags::Tags::moved(&old, &new, cx);
                // Naming the folder "Nova pasta" just made is part of making
                // it; any other rename is a batch of its own for Ctrl+Z.
                if !rename_created(original_path, &renamed) {
                    remember(Undo::Renamed(vec![(
                        original_path.to_path_buf(),
                        renamed.clone(),
                    )]));
                }
                self.reload(cx);
                self.set_status(
                    StatusLevel::Info,
                    tf!("Renomeado para {name}", name = final_name),
                );
            }
            Err(error) => {
                self.set_status(
                    StatusLevel::Error,
                    tf!("Não foi possível renomear: {error}", error = error),
                );
            }
        }
        cx.notify();
    }

    /// Starts the rename of the folder `create_new_folder` made, once it is in
    /// the listing. Called by it and on each frame.
    pub(crate) fn take_rename_when_listed(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.listing_pending || self.rename_when_listed.is_none() {
            return;
        }
        let Some(path) = self.rename_when_listed.take() else {
            return;
        };
        if let Some(index) = self
            .filtered_entries
            .iter()
            .position(|entry| super::same_path(&entry.path, &path))
        {
            self.select_single(index);
            self.scroll_entry_into_view(index);
            self.begin_rename(index, window, cx);
        }
    }

    /// A plain click on the item that was already the only one selected
    /// renames it after the double-click time, as in the Explorer, unless a
    /// second click (a double click) or another click comes first.
    pub(crate) fn arm_slow_rename(
        &mut self,
        ix: usize,
        click_count: usize,
        plain: bool,
        was_sole_selection: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.slow_rename = None;
        if click_count != 1 || !plain || !was_sole_selection || self.renaming.is_some() {
            return;
        }
        let Some(path) = self
            .filtered_entries
            .get(ix)
            .map(|entry| entry.path.clone())
        else {
            return;
        };
        let delay = double_click_time();
        self.slow_rename = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(delay).await;
            this.update_in(cx, |pane, window, cx| {
                pane.slow_rename = None;
                let Some(index) = pane
                    .filtered_entries
                    .iter()
                    .position(|entry| entry.path == path)
                else {
                    return;
                };
                if pane.selection.len() == 1 && pane.is_selected(index) && pane.renaming.is_none() {
                    pane.begin_rename(index, window, cx);
                    cx.notify();
                }
            })
            .ok();
        }));
    }

    /// Creates a new folder in the current directory and immediately starts an
    /// inline rename of it so the user can name it (§1, §6).
    pub(crate) fn create_new_folder(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let parent = PathBuf::from(&self.cwd);
        let name = ops::unique_name(&parent, t!("Nova pasta"));
        match ops::create_dir(&parent, &name) {
            Ok(path) => {
                remember(Undo::Created(vec![path.clone()]));
                // The listing is read on its own thread and may not have the
                // folder yet: it is selected, scrolled to and put in rename
                // mode once it does (`take_rename_when_listed`), instead of
                // being looked for here and, not found, left for the user to
                // hunt down.
                self.reload(cx);
                let created = path.to_string_lossy().into_owned();
                self.reveal(&created);
                self.rename_when_listed = Some(created);
                self.take_rename_when_listed(window, cx);
                cx.notify();
            }
            Err(error) => {
                self.set_status(
                    StatusLevel::Error,
                    tf!("Não foi possível criar a pasta: {error}", error = error),
                );
                cx.notify();
            }
        }
    }
}

// The system's double-click time: a second click within it is a double
// click, and only after it does a click on the selected item rename.
#[cfg(windows)]
#[allow(unsafe_code)]
pub(crate) fn double_click_time() -> std::time::Duration {
    let millis = unsafe { windows::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime() };
    std::time::Duration::from_millis(u64::from(millis.clamp(200, 2000)))
}

#[cfg(not(windows))]
pub(crate) fn double_click_time() -> std::time::Duration {
    std::time::Duration::from_millis(500)
}

#[cfg(test)]
// Staging real entries on disk is the point of these; there is no UI thread in
// a test binary to keep responsive.
#[allow(clippy::unwrap_used, clippy::disallowed_methods)]
mod overwrite_recovery_tests {
    use super::{drop_staged, restore_staged};
    // Only the Unix tests can stage an unreadable tree.
    #[cfg(unix)]
    use super::{ClipMode, apply_to_free_name, drop_claimed, overwrite_apply};
    #[cfg(unix)]
    use nohrs_services::fs::ops;

    // A directory holding something a copy cannot read, so a no-replace copy of
    // it stops at a known point *after* it has created the destination. That is
    // the side of the claim a name taken inside the tree falls on too, which is
    // the case this recovery exists for and the one no test can stage without a
    // second process.
    #[cfg(unix)]
    fn uncopyable_tree(at: &std::path::Path) -> std::os::unix::net::UnixListener {
        std::fs::create_dir(at).unwrap();
        std::fs::write(at.join("page.txt"), "new").unwrap();
        std::os::unix::net::UnixListener::bind(at.join("ipc")).unwrap()
    }

    #[cfg(unix)]
    #[test]
    fn a_paste_to_a_free_name_does_not_leave_its_half_copy_behind() {
        // Nothing is waiting to be restored to this name, so the partial blocks
        // no rollback — but it stands in the listing looking like a finished
        // folder, and it keeps the name, so the retry is numbered around it and
        // the wreckage stays for good.
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("report");
        let _socket = uncopyable_tree(&source);

        let work = dir.path().join("work");
        std::fs::create_dir(&work).unwrap();
        let dst = work.join("report");

        assert!(apply_to_free_name(ClipMode::Copy, &source, &dst).is_err());

        assert!(!dst.exists(), "the half-copy has to go");
        assert!(
            source.join("page.txt").is_file(),
            "and it is only ever the copy that goes — the source is intact"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_paste_whose_name_was_taken_first_leaves_that_alone() {
        // The same call, the other side of the claim. `free_destination` looked
        // and the name was free; by the time the copy reached for it someone
        // else had it. What is there was never this paste's to remove.
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("report");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("page.txt"), "mine").unwrap();

        let work = dir.path().join("work");
        std::fs::create_dir(&work).unwrap();
        let dst = work.join("report");
        std::fs::create_dir(&dst).unwrap();
        std::fs::write(dst.join("theirs.txt"), "theirs").unwrap();

        assert!(apply_to_free_name(ClipMode::Copy, &source, &dst).is_err());

        assert_eq!(
            std::fs::read_to_string(dst.join("theirs.txt")).unwrap(),
            "theirs"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_destination_the_write_never_took_is_left_alone() {
        // The case that makes reading `AlreadyExists` as "this is mine" so
        // expensive: the name was taken before the copy reached for it, and
        // removing what is there destroys a directory this application never
        // created, with whatever the process that did create it had written.
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        std::fs::create_dir(&source).unwrap();
        std::fs::write(source.join("mine.txt"), "mine").unwrap();

        let theirs = dir.path().join("theirs");
        std::fs::create_dir(&theirs).unwrap();
        std::fs::write(theirs.join("theirs.txt"), "theirs").unwrap();

        let failure = ops::copy_path_no_replace(&source, &theirs).unwrap_err();
        drop_claimed(&failure, &theirs);

        assert_eq!(
            std::fs::read_to_string(theirs.join("theirs.txt")).unwrap(),
            "theirs"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_destination_the_write_did_take_is_cleared_away() {
        // And the half-written tree that *is* ours goes, because leaving it is
        // what blocks the rollback that puts the original back.
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source");
        let _socket = uncopyable_tree(&source);

        let dst = dir.path().join("dst");
        let failure = ops::copy_path_no_replace(&source, &dst).unwrap_err();
        assert!(dst.is_dir(), "the copy has to get past creating it");
        drop_claimed(&failure, &dst);

        assert!(!dst.exists());
    }

    #[cfg(unix)]
    #[test]
    fn an_overwrite_that_cannot_build_puts_the_original_back() {
        // The whole recovery end to end: the original is moved aside, the
        // replacement fails once the staging path is its own, the staging goes,
        // and the original comes back to the name the user was looking at —
        // with nothing of ours left beside it.
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("report");
        let _socket = uncopyable_tree(&source);

        let work = dir.path().join("work");
        std::fs::create_dir(&work).unwrap();
        let dst = work.join("report");
        std::fs::create_dir(&dst).unwrap();
        std::fs::write(dst.join("page.txt"), "original").unwrap();

        assert!(overwrite_apply(ClipMode::Copy, &source, &dst).is_err());

        assert_eq!(
            std::fs::read_to_string(dst.join("page.txt")).unwrap(),
            "original",
            "the original has to be back where it was"
        );
        let leftovers: Vec<_> = std::fs::read_dir(&work)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.file_name())
            .filter(|name| name != "report")
            .collect();
        assert!(leftovers.is_empty(), "left behind: {leftovers:?}");
    }

    #[test]
    fn a_cut_that_cannot_finish_puts_the_source_back() {
        // The source is *inside* the staging path by then: a cut moves it there
        // before the destination is taken. Deleting it — which is the right
        // thing for a copy's staging — would destroy the only copy there is.
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("notes.txt");
        let staged = dir.path().join(".notes.txt.nohrs-new");
        std::fs::write(&staged, "payload").unwrap();

        restore_staged(&src, &staged);

        assert_eq!(std::fs::read_to_string(&src).unwrap(), "payload");
        assert!(!staged.exists());
    }

    #[test]
    fn a_source_re_created_under_us_does_not_cost_the_staged_one() {
        // Something else taking the source's name back is not a reason to throw
        // away what was moved out of it. The no-replace move refuses, and the
        // only copy stays where it can still be found.
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("notes.txt");
        std::fs::write(&src, "someone else's").unwrap();
        let staged = dir.path().join(".notes.txt.nohrs-new");
        std::fs::write(&staged, "payload").unwrap();

        restore_staged(&src, &staged);

        assert_eq!(std::fs::read_to_string(&staged).unwrap(), "payload");
        assert_eq!(std::fs::read_to_string(&src).unwrap(), "someone else's");
    }

    #[test]
    fn staging_the_source_outlived_is_dropped() {
        // Every way `apply_one` can fail leaves the source where it was, so what
        // is at the staging path is a fragment and goes.
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("notes.txt");
        std::fs::write(&src, "payload").unwrap();
        let staged = dir.path().join(".notes.txt.nohrs-new");
        std::fs::write(&staged, "half a copy").unwrap();

        drop_staged(&staged);

        assert!(!staged.exists());
        assert_eq!(std::fs::read_to_string(&src).unwrap(), "payload");
    }
}

#[cfg(test)]
mod op_report_tests {
    use super::{Applied, OpReport};
    use crate::explorer::types::StatusLevel;
    use std::path::PathBuf;

    // The batch these describe cannot be staged end to end — reaching
    // `Leftover::WholeDestination` needs a `rename` that fails with `EXDEV`, a
    // copy that completes, and a `remove_dir_all` that gives up partway, three
    // conditions that will not hold together inside a tmpdir. What decides the
    // report, though, is not how the leftover arose but that it did, so the
    // reporting is pinned here on its own.

    #[test]
    fn a_batch_with_nothing_to_say_reports_the_success_label() {
        let report = OpReport::default();
        let (level, text) = report.footer(3, "3 item(s) moved");
        assert_eq!(level, StatusLevel::Info);
        assert_eq!(text, "3 item(s) moved");
    }

    #[test]
    fn a_leftover_is_not_counted_as_a_failure_and_names_where_it_is() {
        // The whole of #283: this batch did what was asked, so "1 of 3 failed"
        // would be untrue and would put the source back on the clipboard. And
        // the success label alone would be untrue the other way, because part of
        // the source is still sitting at /src/report.
        let mut report = OpReport::default();
        report.applied(Applied::Cleanly);
        report.applied(Applied::leftover(
            "/src/report was copied to /dst/report, but the original could not be fully removed"
                .into(),
            PathBuf::from("/src/report"),
        ));
        let (level, text) = report.footer(3, "3 item(s) moved");
        assert_eq!(level, StatusLevel::Warning);
        assert!(text.starts_with("3 item(s) moved — "), "got: {text}");
        // The message carries its own "but"; a second one from the join read as
        // "moved, but /a was copied to /b, but the original ...".
        assert_eq!(text.matches("but").count(), 1, "got: {text}");
        assert!(text.contains("/src/report"), "the source is named: {text}");
        assert!(
            text.contains("/dst/report"),
            "so is the destination: {text}"
        );
    }

    #[test]
    fn several_leftovers_drop_their_sentences_and_keep_their_places() {
        // One line of footer cannot carry two whole messages, but a count on its
        // own is the `warn!`-only behaviour this level exists to replace: the log
        // is not a channel the user of a file manager has. So the sentences go
        // and the paths stay, because the path is the half that can be acted on.
        let mut report = OpReport::default();
        report.applied(Applied::leftover("first".into(), PathBuf::from("/src/one")));
        report.applied(Applied::leftover(
            "second".into(),
            PathBuf::from("/src/two"),
        ));
        let (level, text) = report.footer(2, "2 item(s) moved");
        assert_eq!(level, StatusLevel::Warning);
        assert!(text.contains('2'), "got: {text}");
        assert!(text.contains("/src/one"), "got: {text}");
        assert!(text.contains("/src/two"), "got: {text}");
    }

    #[test]
    fn one_operation_can_strand_two_things_and_names_both() {
        // An overwrite builds a staging copy and keeps a backup of the entry it
        // replaces, and either can fail to be cleared. Keeping only the first
        // would send the user to one scratch path and leave the other where
        // nothing points at it — the scratch names are hidden, so a path the
        // footer omits is one nothing else will ever mention.
        let applied = Applied::leftover(
            "/dst/x was replaced, but the staging copy at /dst/.x.nohrs-new could not be cleared: \
             denied"
                .into(),
            PathBuf::from("/dst/.x.nohrs-new"),
        )
        .and(
            "the copy of the original kept at /dst/.x.nohrs-old could not be removed after \
             replacing /dst/x: denied"
                .into(),
            PathBuf::from("/dst/.x.nohrs-old"),
        );
        let mut report = OpReport::default();
        report.applied(applied);
        let (level, text) = report.footer(1, "1 item(s) pasted");
        assert_eq!(level, StatusLevel::Warning);
        assert!(text.contains("/dst/.x.nohrs-new"), "got: {text}");
        assert!(text.contains("/dst/.x.nohrs-old"), "got: {text}");
        // The two messages both open on "/dst/x was replaced, but", which the
        // footer used to say twice before reaching either path.
        assert!(
            !text.contains("foi substituído, mas"),
            "the sentences are what gets dropped, not the paths: {text}"
        );
        // One item, two leftovers: the count belongs to the places, and saying
        // it against the items read as "1 item(s) pasted — 2 left something
        // behind".
        assert!(text.contains("em 2 lugares"), "got: {text}");
        assert!(text.starts_with("1 item(s) pasted"), "got: {text}");
    }

    #[test]
    fn a_failure_outranks_a_leftover_in_the_same_batch() {
        // Reporting the leftover here would read as though the batch went fine,
        // and the entry that did not move would go unmentioned. The leftover is
        // still in the log; the footer has one line and a failure has first call
        // on it.
        let mut report = OpReport::default();
        report.applied(Applied::leftover(
            "a leftover".into(),
            PathBuf::from("/src/x"),
        ));
        report.failed("/src/other: permission denied".into());
        let (level, text) = report.footer(2, "2 item(s) moved");
        assert_eq!(level, StatusLevel::Error);
        assert_eq!(text, "1 de 2 falharam");
    }
}

#[cfg(test)]
mod rename_selection_tests {
    use super::rename_selection_end;

    #[test]
    fn selects_the_name_without_its_extension() {
        assert_eq!(rename_selection_end("notes.txt", false), 5);
        assert_eq!(rename_selection_end("Cargo.toml", false), 5);
    }

    #[test]
    fn keeps_only_the_final_extension_out_of_the_selection() {
        // "archive.tar" stays selected; typing replaces it and ".gz" survives.
        assert_eq!(rename_selection_end("archive.tar.gz", false), 11);
    }

    #[test]
    fn selects_extensionless_names_whole() {
        assert_eq!(rename_selection_end("Makefile", false), 8);
    }

    #[test]
    fn selects_dotfiles_whole() {
        // `file_stem` treats a leading dot as part of the name; keying off the
        // last `.` instead would select nothing here.
        assert_eq!(rename_selection_end(".gitignore", false), 10);
    }

    #[test]
    fn selects_directories_whole_even_with_dots() {
        assert_eq!(rename_selection_end("my.folder", true), 9);
    }

    #[test]
    fn counts_utf16_units_not_bytes() {
        // The input's text API indexes in UTF-16, so a multi-byte stem must not
        // be measured in bytes (12 here) or chars alone.
        assert_eq!(rename_selection_end("日本語.txt", false), 3);
        // Astral-plane characters take two UTF-16 units each.
        assert_eq!(rename_selection_end("🎉🎉.txt", false), 4);
    }
}

impl ExplorerPane {
    /// "Copy as path": the selection's full paths as text, each in quotes and
    /// on its own line, as Explorer's Ctrl+Shift+C puts them. With nothing
    /// selected, the folder on screen.
    pub(crate) fn copy_paths_as_text(&mut self, cx: &mut Context<Self>) {
        let mut paths = self.selected_paths();
        if paths.is_empty() && std::path::Path::new(&self.cwd).is_absolute() {
            paths.push(self.cwd.clone());
        }
        if paths.is_empty() {
            return;
        }
        let text = paths
            .iter()
            .map(|path| format!("\"{path}\""))
            .collect::<Vec<_>>()
            .join("\r\n");
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        let message = i18n::plural(
            paths.len(),
            k!("Caminho copiado"),
            k!("{n} caminhos copiados"),
        );
        self.set_status(StatusLevel::Info, message);
        cx.notify();
    }

    /// Opens each selected item in a tab of its own: a folder shows itself, a
    /// file shows its folder with it selected (and previewed).
    pub(crate) fn open_selection_in_new_tab(&mut self, cx: &mut Context<Self>) {
        let selected = self.selected_paths();
        for path in selected {
            let is_dir = self
                .filtered_entries
                .iter()
                .find(|entry| entry.path == path)
                .is_some_and(|entry| entry.kind == "dir");
            self.open_path_in_new_tab(&path, is_dir, false, cx);
        }
    }

    /// Asks the page for a new tab on `path` (a folder), or on the folder
    /// holding it with it selected.
    pub(crate) fn open_path_in_new_tab(
        &mut self,
        path: &str,
        is_dir: bool,
        middle: bool,
        cx: &mut Context<Self>,
    ) {
        let event = if is_dir {
            PaneEvent::OpenInNewTab {
                folder: path.to_string(),
                select: None,
                middle,
            }
        } else {
            let Some(parent) = std::path::Path::new(path).parent() else {
                return;
            };
            PaneEvent::OpenInNewTab {
                folder: parent.to_string_lossy().into_owned(),
                select: Some(path.to_string()),
                middle,
            }
        };
        cx.emit(event);
    }

    /// The middle button on the item at `ix`: a tab of its own, as in a
    /// browser.
    pub(crate) fn open_entry_in_new_tab(&mut self, ix: usize, cx: &mut Context<Self>) {
        let Some(entry) = self.filtered_entries.get(ix) else {
            return;
        };
        let (path, is_dir) = (entry.path.clone(), entry.kind == "dir");
        self.open_path_in_new_tab(&path, is_dir, true, cx);
    }

    /// Explorer's Properties window for the selection.
    pub(crate) fn show_selection_properties(&mut self, cx: &mut Context<Self>) {
        let paths: Vec<PathBuf> = self
            .selected_paths()
            .into_iter()
            .map(PathBuf::from)
            .collect();
        if paths.is_empty() {
            return;
        }
        if let Err(error) = super::drag::show_properties(&paths) {
            self.set_status(
                StatusLevel::Error,
                tf!(
                    "Não foi possível abrir as propriedades: {error}",
                    error = error
                ),
            );
            cx.notify();
        }
    }
}

// What a batch was, for the menu to name what Ctrl+Z would take back.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum UndoKind {
    Trashed,
    Moved,
    Renamed,
    Copied,
    Created,
}

// One batch Ctrl+Z can take back and Ctrl+Y do again.
enum Undo {
    /// Sent to the Recycle Bin, by original path.
    Trashed(Vec<PathBuf>),
    /// Moved, as (from, to). A move over an item it replaced is not here:
    /// moving it back would not bring the replaced one back.
    Moved(Vec<(PathBuf, PathBuf)>),
    /// Renamed, as (old path, new path): a move in the same folder, named
    /// apart in the menu.
    Renamed(Vec<(PathBuf, PathBuf)>),
    /// Copies a paste or a drop made, by their path. One that replaced an
    /// item already there is not here, as for a move.
    Copied(Vec<PathBuf>),
    /// A new folder, by its path (the name it was given, once renamed).
    Created(Vec<PathBuf>),
}

impl Undo {
    fn kind(&self) -> UndoKind {
        match self {
            Undo::Trashed(_) => UndoKind::Trashed,
            Undo::Moved(_) => UndoKind::Moved,
            Undo::Renamed(_) => UndoKind::Renamed,
            Undo::Copied(_) => UndoKind::Copied,
            Undo::Created(_) => UndoKind::Created,
        }
    }

    // A batch of single items of this kind.
    fn items(kind: UndoKind, paths: Vec<PathBuf>) -> Self {
        match kind {
            UndoKind::Copied => Undo::Copied(paths),
            UndoKind::Created => Undo::Created(paths),
            _ => Undo::Trashed(paths),
        }
    }

    // A batch of (from, to) pairs of this kind.
    fn pairs(kind: UndoKind, pairs: Vec<(PathBuf, PathBuf)>) -> Self {
        match kind {
            UndoKind::Renamed => Undo::Renamed(pairs),
            _ => Undo::Moved(pairs),
        }
    }

    fn is_empty(&self) -> bool {
        match self {
            Undo::Trashed(paths) | Undo::Copied(paths) | Undo::Created(paths) => paths.is_empty(),
            Undo::Moved(pairs) | Undo::Renamed(pairs) => pairs.is_empty(),
        }
    }
}

// What was done, a batch per operation, newest last, and what Ctrl+Z took
// back since, for Ctrl+Y. Kept for the app rather than a tab.
#[derive(Default)]
struct Histories {
    undo: Vec<Undo>,
    redo: Vec<Undo>,
}

#[cfg(not(test))]
static HISTORY: std::sync::Mutex<Histories> = std::sync::Mutex::new(Histories {
    undo: Vec::new(),
    redo: Vec::new(),
});
const UNDO_DEPTH: usize = 30;

#[cfg(not(test))]
fn with_history<R>(f: impl FnOnce(&mut Histories) -> R) -> Option<R> {
    HISTORY.lock().ok().map(|mut history| f(&mut history))
}

// One history per test thread: tests run side by side, and one deleting
// files would hand its batch to another's Ctrl+Z.
#[cfg(test)]
thread_local! {
    static HISTORY: std::cell::RefCell<Histories> = std::cell::RefCell::new(Histories::default());
}

#[cfg(test)]
fn with_history<R>(f: impl FnOnce(&mut Histories) -> R) -> Option<R> {
    Some(HISTORY.with(|history| f(&mut history.borrow_mut())))
}

fn push_capped(list: &mut Vec<Undo>, batch: Undo) {
    list.push(batch);
    let excess = list.len().saturating_sub(UNDO_DEPTH);
    list.drain(..excess);
}

// A new operation: on the undo list, and the redo list is gone.
fn remember(batch: Undo) {
    if batch.is_empty() {
        return;
    }
    with_history(|history| {
        push_capped(&mut history.undo, batch);
        history.redo.clear();
    });
}

/// Renames made together (several items at once), as one step of Ctrl+Z.
pub(crate) fn remember_renames(pairs: Vec<(PathBuf, PathBuf)>) {
    remember(Undo::Renamed(pairs));
}

// A batch played by Ctrl+Z (`back`) goes on the redo list; by Ctrl+Y, back
// on the undo list. Neither empties the other.
fn file_replayed(batch: Undo, back: bool) {
    if batch.is_empty() {
        return;
    }
    with_history(|history| {
        push_capped(
            if back {
                &mut history.redo
            } else {
                &mut history.undo
            },
            batch,
        )
    });
}

// A rename of the folder just made by "Nova pasta" is part of making it:
// one Ctrl+Z takes the folder away instead of first giving it its old name.
// Returns whether the rename was taken in.
fn rename_created(old: &Path, new: &Path) -> bool {
    with_history(|history| match history.undo.last_mut() {
        Some(Undo::Created(paths)) if paths.len() == 1 && same_place(&paths[0], old) => {
            paths[0] = new.to_path_buf();
            history.redo.clear();
            true
        }
        _ => false,
    })
    .unwrap_or(false)
}

// Windows paths compare without case, and the Recycle Bin may hand one back
// with its own capitals.
fn same_place(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
}

// Puts each path of a batch back from the Recycle Bin: the newest item that
// came from there, as the same name may have been deleted before.
fn restore_batch(batch: Vec<PathBuf>) -> (OpReport, Vec<PathBuf>) {
    let mut report = OpReport::default();
    let mut restored = Vec::new();
    let mut store =
        match nohrs_services::fs::trash::default_store(nohrs_services::fs::trash::open_ledger) {
            Ok(store) => store,
            Err(error) => {
                report.failed(error.to_string());
                return (report, restored);
            }
        };
    let items = match store.list() {
        Ok(items) => items,
        Err(error) => {
            report.failed(error.to_string());
            return (report, restored);
        }
    };
    for path in &batch {
        // The newest item that came from there: the same name may have
        // been deleted before.
        let item = items
            .iter()
            .filter(|item| same_place(&item.original_path, path))
            .max_by_key(|item| item.deleted_at_unix);
        match item {
            Some(item) => match store.restore(item) {
                Ok(()) => restored.push(path.clone()),
                Err(error) => report.failed(format!("{}: {error}", path.display())),
            },
            None => report.failed(tf!(
                "{path}: não está mais na Lixeira",
                path = path.display()
            )),
        }
    }
    (report, restored)
}
