//! Picking files for a browser's "Open" dialog from Nohrs.
//!
//! The dialog itself cannot be replaced: it runs inside the browser, and
//! browsers refuse injected code. So Nohrs watches for one to come up (a
//! WinEvent hook, the same mechanism screen readers and Listary use), comes
//! forward with a banner, and when a file is chosen here writes its path into
//! the dialog's file name box and presses Open, as a person typing it would.
//!
//! While Nohrs picks, the dialog is moved off the screen, so only Nohrs is on
//! it (the owner asked for that, 07/11/2026: both windows came up, and the
//! one to use was not obvious). Moved, not hidden: a dialog hidden from the
//! start never loaded its folder view (a blank box with a spinner), and
//! until it has, it ignores Open; the file name went in and nothing was
//! sent. It comes back, where it was, whenever Nohrs stops picking without
//! sending: Esc or Cancel, a file the dialog did not take, the explorer
//! window gone, the app quitting or panicking. A hidden dialog nobody gives
//! back would leave the browser locked behind it.

use std::sync::Mutex;
use std::time::Duration;

use gpui::{Context, WeakEntity, Window};

use crate::root::RootView;

/// A browser file dialog waiting for Nohrs to choose for it.
#[derive(Clone, Debug)]
pub struct Dialog {
    hwnd: isize,
    /// The dialog's title ("Open", "Abrir"…), for the banner.
    pub title: String,
    /// The browser's executable name, for the banner.
    pub browser: String,
}

// The dialog being picked for. A global rather than a field of the root view:
// the explorer tab that activates a file is a different entity, and it only
// needs to know whether to hand the file over instead of opening it.
static PICKING: Mutex<Option<Dialog>> = Mutex::new(None);

impl Dialog {
    /// The dialog's window, which tells one dialog from the next.
    pub fn id(&self) -> isize {
        self.hwnd
    }
}

/// The dialog being picked for, if any.
pub fn picking() -> Option<Dialog> {
    PICKING.lock().ok().and_then(|picking| picking.clone())
}

fn set_picking(dialog: Option<Dialog>) {
    if let Ok(mut picking) = PICKING.lock() {
        *picking = dialog;
    }
}

// The dialog last given back to the user. Showing it again brings it to the
// foreground, which the hook reports like a dialog just opened; without this
// Nohrs would take it and hide it again at once.
static RELEASED: Mutex<Option<isize>> = Mutex::new(None);

fn released(hwnd: isize) -> bool {
    RELEASED
        .lock()
        .ok()
        .is_some_and(|released| *released == Some(hwnd))
}

// Shows the dialog again, in front, and leaves it to the user.
fn give_back(hwnd: isize) {
    if let Ok(mut released) = RELEASED.lock() {
        *released = Some(hwnd);
    }
    platform::show(hwnd);
}

/// Hands `paths` to the dialog being picked for and closes it with Open.
/// Returns `false` when it could not: the dialog is gone, or it did not take
/// the file, in which case it is back on screen for the user.
pub fn deliver(paths: &[String]) -> bool {
    let Some(dialog) = picking() else {
        return false;
    };
    set_picking(None);
    if paths.is_empty() || !platform::fill_and_open(dialog.hwnd, paths) {
        if platform::is_open(dialog.hwnd) {
            give_back(dialog.hwnd);
        }
        return false;
    }
    // Open closes the dialog. One still there a moment later is showing
    // something (a file it refuses, a question): it needs the user, so it
    // comes back.
    let hwnd = dialog.hwnd;
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(1200));
        if platform::is_open(hwnd) {
            give_back(hwnd);
        }
    });
    true
}

/// Stops picking and gives the dialog back to the user, untouched.
pub fn cancel() {
    if let Some(dialog) = picking() {
        set_picking(None);
        give_back(dialog.hwnd);
    }
}

// Moves the dialog off the screen, and again over its first second. The hook
// reports it as soon as it takes the foreground, before the dialog has
// finished coming up, and its own placing (centred on the browser) came
// after a single move and undid it (07/11/2026). Stops as soon as Nohrs
// stops picking for it, so a dialog given back is left alone.
fn keep_hidden(hwnd: isize) {
    platform::hide(hwnd);
    std::thread::spawn(move || {
        for wait in [30, 70, 150, 250, 500] {
            std::thread::sleep(Duration::from_millis(wait));
            if picking().is_none_or(|dialog| dialog.hwnd != hwnd) {
                return;
            }
            if !platform::is_parked(hwnd) {
                platform::hide(hwnd);
            }
        }
    });
}

// ---- File types ----

/// One entry of the dialog's list of file types ("Imagens (*.png;*.jpg)").
#[derive(Clone, Debug, PartialEq)]
pub struct FileType {
    /// What the dialog's list shows for it.
    pub label: String,
    /// What it lets through; `None` for every file.
    pub patterns: Option<Vec<String>>,
}

/// The dialog's file types, read from its own list, and the one it shows.
/// Empty while it has not filled the list, or when it has none.
pub fn file_types(hwnd: isize) -> (Vec<FileType>, usize) {
    let (labels, chosen) = platform::file_types(hwnd);
    let types = labels
        .into_iter()
        .map(|label| FileType {
            patterns: parse_patterns(&label),
            label,
        })
        .collect();
    (types, chosen)
}

/// The patterns in a type's label, between its last parentheses; `None`
/// when it has none or they let everything through.
pub fn parse_patterns(label: &str) -> Option<Vec<String>> {
    let open = label.rfind('(')?;
    let close = open + label[open..].find(')')?;
    parse_typed_patterns(&label[open + 1..close])
}

/// Patterns separated by `;`, `,` or spaces, as a label lists them or a
/// person types them; `None` when they let everything through.
pub fn parse_typed_patterns(text: &str) -> Option<Vec<String>> {
    let patterns: Vec<String> = text
        .split([';', ',', ' '])
        .map(str::trim)
        .filter(|pattern| !pattern.is_empty())
        .map(str::to_lowercase)
        .collect();
    if patterns.is_empty()
        || patterns
            .iter()
            .any(|pattern| pattern == "*" || pattern == "*.*")
    {
        return None;
    }
    Some(patterns)
}

/// Whether `name` fits one of `patterns`: `*` any run, `?` one character,
/// without case.
pub fn matches_any(patterns: &[String], name: &str) -> bool {
    let name: Vec<char> = name.to_lowercase().chars().collect();
    patterns.iter().any(|pattern| {
        let pattern: Vec<char> = pattern.chars().collect();
        wildcard(&pattern, &name)
    })
}

fn wildcard(pattern: &[char], name: &[char]) -> bool {
    let (mut p, mut n) = (0, 0);
    let mut star: Option<usize> = None;
    let mut mark = 0;
    while n < name.len() {
        if p < pattern.len() && (pattern[p] == '?' || pattern[p] == name[n]) {
            p += 1;
            n += 1;
        } else if p < pattern.len() && pattern[p] == '*' {
            star = Some(p);
            mark = n;
            p += 1;
        } else if let Some(at) = star {
            p = at + 1;
            mark += 1;
            n = mark;
        } else {
            return false;
        }
    }
    while p < pattern.len() && pattern[p] == '*' {
        p += 1;
    }
    p == pattern.len()
}

// What the listings show files by while picking (`picking_bar`).
static PATTERNS: Mutex<Option<Vec<String>>> = Mutex::new(None);

/// Sets what the listings show files by while picking; `None` shows all.
pub fn set_patterns(patterns: Option<Vec<String>>) {
    if let Ok(mut held) = PATTERNS.lock() {
        *held = patterns;
    }
}

/// The patterns files are listed by, while a dialog waits; `None` lists all.
pub fn listing_patterns() -> Option<Vec<String>> {
    picking()?;
    PATTERNS.lock().ok().and_then(|patterns| patterns.clone())
}

// ---- Switching to an Explorer window's folder, as Listary does ----

// The Explorer window that last came to the front, counted, and the count
// already acted on. A visit to an Explorer window while a dialog waits, then
// back to Nohrs, takes Nohrs to that window's folder.
static EXPLORER_SEEN: Mutex<(u64, isize)> = Mutex::new((0, 0));
static EXPLORER_TAKEN: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn note_explorer(hwnd: isize) {
    if let Ok(mut seen) = EXPLORER_SEEN.lock() {
        seen.0 += 1;
        seen.1 = hwnd;
    }
}

// Visits from before the dialog came up do not count.
fn forget_explorer_visits() {
    if let Ok(seen) = EXPLORER_SEEN.lock() {
        EXPLORER_TAKEN.store(seen.0, std::sync::atomic::Ordering::Relaxed);
    }
}

/// The Explorer window visited since Nohrs last looked, while a dialog waits.
pub fn take_explorer_visit() -> Option<isize> {
    picking()?;
    let (count, hwnd) = *EXPLORER_SEEN.lock().ok()?;
    let taken = EXPLORER_TAKEN.swap(count, std::sync::atomic::Ordering::Relaxed);
    (taken != count && platform::is_open(hwnd)).then_some(hwnd)
}

/// The Open/Save dialog in front, of any program, when there is one: where
/// the quick search sends what is chosen (Listary's quick switch).
pub fn foreground_file_dialog() -> Option<isize> {
    platform::foreground_file_dialog()
}

/// Puts `path` in the dialog's file name box and presses Open: a file is
/// chosen, a folder is gone into. The dialog comes to the front.
pub fn fill_dialog(hwnd: isize, path: &str) -> bool {
    if !platform::fill_and_open(hwnd, &[path.to_string()]) {
        return false;
    }
    platform::bring_forward(hwnd);
    true
}

/// The folder an Explorer window shows. Blocks on the Shell: call it off the
/// UI thread.
pub fn explorer_folder(hwnd: isize) -> Option<String> {
    platform::explorer_folder(hwnd)
}

// Gives the hidden dialog back from a panic: no locks waited on, since the
// panic may have happened holding one.
fn give_back_on_panic() {
    if let Ok(picking) = PICKING.try_lock()
        && let Some(dialog) = picking.as_ref()
    {
        platform::show(dialog.hwnd);
    }
}

/// Starts watching for browser file dialogs. While one is open Nohrs is in
/// picking mode; the banner and the poll that ends it run on the root view.
pub fn watch(_window: &mut Window, cx: &mut Context<RootView>) {
    let (sender, receiver) = async_channel::unbounded::<Dialog>();
    if !platform::start_hook(sender) {
        return;
    }
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        give_back_on_panic();
        previous(info);
    }));
    cx.on_app_quit(|_root, _cx| {
        cancel();
        async {}
    })
    .detach();
    cx.spawn(
        async move |_this: WeakEntity<RootView>, cx: &mut gpui::AsyncApp| {
            loop {
                // Either a dialog came up, or it is time to check that the one
                // being picked for still exists: the user may have chosen in the
                // dialog itself, or cancelled it.
                let arrived = next_dialog_or_tick(&receiver, cx).await;
                let changed = match arrived {
                    // One the user was given back stays theirs.
                    Some(dialog) if released(dialog.hwnd) => false,
                    Some(dialog) => {
                        forget_explorer_visits();
                        set_picking(Some(dialog.clone()));
                        keep_hidden(dialog.hwnd);
                        true
                    }
                    None => match picking() {
                        Some(dialog) if !platform::is_open(dialog.hwnd) => {
                            set_picking(None);
                            true
                        }
                        // No explorer window left to pick in: the dialog
                        // would stay hidden for good.
                        Some(_) if crate::windows::with_target(cx, |_, _, _| ()).is_none() => {
                            cancel();
                            false
                        }
                        // Put back on screen by the browser or the dialog itself.
                        Some(dialog) if !platform::is_parked(dialog.hwnd) => {
                            platform::hide(dialog.hwnd);
                            false
                        }
                        _ => false,
                    },
                };
                // In the window used last (`crate::windows`).
                if changed
                    && crate::windows::with_target(cx, |root, window, cx| {
                        root.show_explorer(cx);
                        if picking().is_some() {
                            window.activate_window();
                        }
                        cx.notify();
                    })
                    .is_none()
                {
                    tracing::debug!("no explorer window to pick in");
                }
            }
        },
    )
    .detach();
}

// Waits for the next dialog, but no longer than the poll interval.
async fn next_dialog_or_tick(
    receiver: &async_channel::Receiver<Dialog>,
    cx: &mut gpui::AsyncApp,
) -> Option<Dialog> {
    let timer = cx.background_executor().timer(Duration::from_millis(400));
    let next = receiver.recv();
    first_of(next, timer).await
}

async fn first_of(
    next: impl std::future::Future<Output = Result<Dialog, async_channel::RecvError>>,
    timer: impl std::future::Future<Output = ()>,
) -> Option<Dialog> {
    use std::pin::pin;
    use std::task::Poll;
    let mut next = pin!(next);
    let mut timer = pin!(timer);
    std::future::poll_fn(|context| {
        if let Poll::Ready(dialog) = next.as_mut().poll(context) {
            return Poll::Ready(dialog.ok());
        }
        if timer.as_mut().poll(context).is_ready() {
            return Poll::Ready(None);
        }
        Poll::Pending
    })
    .await
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod platform {
    //! The hook and the dialog automation. `unsafe` is confined here: none of
    //! this has a safe binding.

    use std::sync::OnceLock;

    use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, WPARAM};
    use windows::Win32::System::Threading::{
        OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
        QueryFullProcessImageNameW,
    };
    use windows::Win32::UI::Accessibility::{HWINEVENTHOOK, SetWinEventHook};
    use windows::Win32::UI::WindowsAndMessaging::{
        BN_CLICKED, CB_GETCOUNT, CB_GETCURSEL, CB_GETLBTEXT, CB_GETLBTEXTLEN, DispatchMessageW,
        EVENT_SYSTEM_FOREGROUND, EnumChildWindows, FindWindowExW, GetClassNameW, GetDlgCtrlID,
        GetDlgItem, GetMessageW, GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IDOK,
        IsWindow, MSG, SMTO_ABORTIFHUNG, SMTO_BLOCK, SWP_NOACTIVATE, SWP_NOSIZE, SWP_NOZORDER,
        SendMessageTimeoutW, SendMessageW, SetForegroundWindow, SetWindowPos, TranslateMessage,
        WINEVENT_OUTOFCONTEXT, WINEVENT_SKIPOWNPROCESS, WM_COMMAND, WM_SETTEXT,
    };
    use windows::core::{PCWSTR, PWSTR, w};

    use super::Dialog;

    /// Control id of the file name combo box in the common Open/Save dialog
    /// (`cmb13`); the Vista-style dialog keeps it for compatibility. Its
    /// presence is what tells a file dialog from any other `#32770` box.
    const FILE_NAME_COMBO: i32 = 1148;
    /// The file name edit box of the older dialog layout (`edt1`).
    const FILE_NAME_EDIT: i32 = 1152;

    /// Browsers whose file dialogs Nohrs answers. Helium and other Chromium
    /// builds run as `chrome.exe`.
    const BROWSERS: &[&str] = &[
        "chrome.exe",
        "msedge.exe",
        "zen.exe",
        "firefox.exe",
        "brave.exe",
        "opera.exe",
        "vivaldi.exe",
        "floorp.exe",
        "librewolf.exe",
        "thorium.exe",
        "arc.exe",
    ];

    static SENDER: OnceLock<async_channel::Sender<Dialog>> = OnceLock::new();

    /// More programs to answer, from `NOHRS_FILE_DIALOG_APPS` (executable names
    /// separated by commas, e.g. `Adobe Premiere Pro.exe,powershell.exe`).
    /// Read once: the hook runs for the life of the process.
    fn extra_apps() -> &'static [String] {
        static EXTRA: OnceLock<Vec<String>> = OnceLock::new();
        EXTRA.get_or_init(|| {
            std::env::var("NOHRS_FILE_DIALOG_APPS")
                .unwrap_or_default()
                .split(',')
                .map(|name| name.trim().to_lowercase())
                .filter(|name| !name.is_empty())
                .collect()
        })
    }

    pub fn start_hook(sender: async_channel::Sender<Dialog>) -> bool {
        if SENDER.set(sender).is_err() {
            return false;
        }
        // An out-of-context hook calls back on the thread that set it, from its
        // message loop, so it gets a thread of its own for the whole run.
        std::thread::Builder::new()
            .name("nohrs-file-dialogs".into())
            .spawn(|| unsafe {
                let hook = SetWinEventHook(
                    EVENT_SYSTEM_FOREGROUND,
                    EVENT_SYSTEM_FOREGROUND,
                    None,
                    Some(on_foreground),
                    0,
                    0,
                    WINEVENT_OUTOFCONTEXT | WINEVENT_SKIPOWNPROCESS,
                );
                if hook.is_invalid() {
                    tracing::warn!("file dialog watch disabled: SetWinEventHook failed");
                    return;
                }
                let mut message = MSG::default();
                while GetMessageW(&mut message, None, 0, 0).as_bool() {
                    _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            })
            .is_ok()
    }

    unsafe extern "system" fn on_foreground(
        _hook: HWINEVENTHOOK,
        _event: u32,
        hwnd: HWND,
        _object: i32,
        _child: i32,
        _thread: u32,
        _time: u32,
    ) {
        if class_of(hwnd) == "CabinetWClass" {
            super::note_explorer(hwnd.0 as isize);
            return;
        }
        if let Some(dialog) = browser_file_dialog(hwnd)
            && let Some(sender) = SENDER.get()
        {
            sender.try_send(dialog).ok();
        }
    }

    fn class_of(hwnd: HWND) -> String {
        let mut class = [0u16; 64];
        let length = unsafe { GetClassNameW(hwnd, &mut class) } as usize;
        String::from_utf16_lossy(&class[..length])
    }

    /// Control id of the file type combo box (`cmb1`). In the dialog of
    /// Windows 10 and 11 it sits deep in the window tree, so it is searched
    /// for rather than asked of the dialog.
    const FILE_TYPE_COMBO: i32 = 1136;

    fn find_child(parent: HWND, id: i32, class: &str) -> Option<HWND> {
        struct Search<'a> {
            id: i32,
            class: &'a str,
            found: Option<HWND>,
        }
        unsafe extern "system" fn visit(hwnd: HWND, lparam: LPARAM) -> windows::core::BOOL {
            let search = unsafe { &mut *(lparam.0 as *mut Search) };
            if unsafe { GetDlgCtrlID(hwnd) } == search.id && class_of(hwnd) == search.class {
                search.found = Some(hwnd);
                return false.into();
            }
            true.into()
        }
        let mut search = Search {
            id,
            class,
            found: None,
        };
        unsafe {
            let _ = EnumChildWindows(
                Some(parent),
                Some(visit),
                LPARAM(&mut search as *mut Search as isize),
            );
        }
        search.found
    }

    // A message to another program's window that gives up rather than hang
    // Nohrs when that program does not answer.
    fn ask(hwnd: HWND, message: u32, wparam: usize, lparam: isize) -> Option<usize> {
        let mut result = 0usize;
        let answered = unsafe {
            SendMessageTimeoutW(
                hwnd,
                message,
                WPARAM(wparam),
                LPARAM(lparam),
                SMTO_ABORTIFHUNG | SMTO_BLOCK,
                300,
                Some(&mut result),
            )
        };
        (answered.0 != 0).then_some(result)
    }

    pub fn file_types(hwnd: isize) -> (Vec<String>, usize) {
        let Some(combo) = find_child(HWND(hwnd as *mut _), FILE_TYPE_COMBO, "ComboBox") else {
            return (Vec::new(), 0);
        };
        let count = ask(combo, CB_GETCOUNT, 0, 0)
            .map(|count| count as isize)
            .unwrap_or(0);
        let mut labels = Vec::new();
        for index in 0..count.clamp(0, 64) as usize {
            let Some(length) = ask(combo, CB_GETLBTEXTLEN, index, 0).map(|length| length as isize)
            else {
                break;
            };
            if !(0..4096).contains(&length) {
                break;
            }
            let mut text = vec![0u16; length as usize + 1];
            if ask(combo, CB_GETLBTEXT, index, text.as_mut_ptr() as isize).is_none() {
                break;
            }
            labels.push(String::from_utf16_lossy(&text[..length as usize]));
        }
        let chosen = ask(combo, CB_GETCURSEL, 0, 0)
            .map(|chosen| chosen as isize)
            .filter(|chosen| *chosen >= 0 && (*chosen as usize) < labels.len())
            .unwrap_or(0) as usize;
        (labels, chosen)
    }

    pub fn explorer_folder(hwnd: isize) -> Option<String> {
        use windows::Win32::System::Com::{
            CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
            CoUninitialize, IServiceProvider,
        };
        use windows::Win32::System::Variant::VARIANT;
        use windows::Win32::UI::Shell::{
            IFolderView, IPersistFolder2, IShellBrowser, IShellWindows, IWebBrowserApp,
            SHGetNameFromIDList, SID_STopLevelBrowser, SIGDN_FILESYSPATH, ShellWindows,
        };
        use windows::core::Interface;
        unsafe {
            let initialized = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
            let folder = (|| -> Option<String> {
                let shell_windows: IShellWindows =
                    CoCreateInstance(&ShellWindows, None, CLSCTX_ALL).ok()?;
                let count = shell_windows.Count().ok()?;
                for index in 0..count {
                    let Ok(item) = shell_windows.Item(&VARIANT::from(index)) else {
                        continue;
                    };
                    let Ok(browser) = item.cast::<IWebBrowserApp>() else {
                        continue;
                    };
                    if browser.HWND().map(|handle| handle.0 as isize).ok() != Some(hwnd) {
                        continue;
                    }
                    let provider: IServiceProvider = browser.cast().ok()?;
                    let shell_browser: IShellBrowser =
                        provider.QueryService(&SID_STopLevelBrowser).ok()?;
                    let view = shell_browser.QueryActiveShellView().ok()?;
                    let folder_view: IFolderView = view.cast().ok()?;
                    let folder: IPersistFolder2 = folder_view.GetFolder().ok()?;
                    let pidl = folder.GetCurFolder().ok()?;
                    let name = SHGetNameFromIDList(pidl, SIGDN_FILESYSPATH);
                    CoTaskMemFree(Some(pidl as *const _));
                    let name = name.ok()?;
                    let path = name.to_string().ok();
                    CoTaskMemFree(Some(name.0 as *const _));
                    return path;
                }
                None
            })();
            if initialized {
                CoUninitialize();
            }
            folder
        }
    }

    pub fn foreground_file_dialog() -> Option<isize> {
        let hwnd = unsafe { windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow() };
        if hwnd.is_invalid() || class_of(hwnd) != "#32770" {
            return None;
        }
        unsafe { GetDlgItem(Some(hwnd), FILE_NAME_COMBO) }.ok()?;
        Some(hwnd.0 as isize)
    }

    fn browser_file_dialog(hwnd: HWND) -> Option<Dialog> {
        if class_of(hwnd) != "#32770" {
            return None;
        }
        unsafe { GetDlgItem(Some(hwnd), FILE_NAME_COMBO) }.ok()?;
        let browser = process_name(hwnd)?;
        if !BROWSERS.contains(&browser.as_str()) && !extra_apps().contains(&browser) {
            return None;
        }
        let mut title = [0u16; 256];
        let length = unsafe { GetWindowTextW(hwnd, &mut title) } as usize;
        Some(Dialog {
            hwnd: hwnd.0 as isize,
            title: String::from_utf16_lossy(&title[..length]),
            browser,
        })
    }

    fn process_name(hwnd: HWND) -> Option<String> {
        let mut pid = 0u32;
        unsafe { GetWindowThreadProcessId(hwnd, Some(&mut pid)) };
        let process = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) }.ok()?;
        let mut path = [0u16; 1024];
        let mut length = path.len() as u32;
        let queried = unsafe {
            QueryFullProcessImageNameW(
                process,
                PROCESS_NAME_WIN32,
                PWSTR(path.as_mut_ptr()),
                &mut length,
            )
        };
        unsafe { CloseHandle(process) }.ok();
        queried.ok()?;
        let path = String::from_utf16_lossy(&path[..length as usize]);
        let name = std::path::Path::new(&path)
            .file_name()?
            .to_string_lossy()
            .to_lowercase();
        Some(name)
    }

    pub fn is_open(hwnd: isize) -> bool {
        unsafe { IsWindow(Some(HWND(hwnd as *mut _))) }.as_bool()
    }

    pub fn bring_forward(hwnd: isize) {
        // Nohrs is in front when this runs, which is what lets it hand the
        // foreground over.
        unsafe { SetForegroundWindow(HWND(hwnd as *mut _)) }
            .ok()
            .ok();
    }

    /// Where parked dialogs go: where Windows itself puts minimized windows,
    /// off every monitor.
    const PARKED: i32 = -32000;

    /// Where the dialog was before it was parked, to put it back there.
    static ORIGIN: std::sync::Mutex<Option<(isize, i32, i32)>> = std::sync::Mutex::new(None);

    fn position(hwnd: isize) -> Option<(i32, i32)> {
        let mut rect = windows::Win32::Foundation::RECT::default();
        unsafe { GetWindowRect(HWND(hwnd as *mut _), &mut rect) }.ok()?;
        Some((rect.left, rect.top))
    }

    fn move_to(hwnd: isize, x: i32, y: i32) {
        unsafe {
            SetWindowPos(
                HWND(hwnd as *mut _),
                None,
                x,
                y,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            )
        }
        .ok();
    }

    pub fn is_parked(hwnd: isize) -> bool {
        position(hwnd).is_none_or(|(x, y)| x <= PARKED / 2 && y <= PARKED / 2)
    }

    /// Takes the dialog off the screen while Nohrs picks for it. It stays
    /// shown, so it loads and takes the file name and Open as it would on
    /// screen. Where it was is kept, each time: the dialog may have placed
    /// itself again since the last time.
    pub fn hide(hwnd: isize) {
        if !is_open(hwnd) || is_parked(hwnd) {
            return;
        }
        if let Some((x, y)) = position(hwnd)
            && let Ok(mut origin) = ORIGIN.lock()
        {
            *origin = Some((hwnd, x, y));
        }
        move_to(hwnd, PARKED, PARKED);
    }

    /// Puts it back on screen where it was, in front. From a panic too, so
    /// the lock is only tried.
    pub fn show(hwnd: isize) {
        if !is_open(hwnd) {
            return;
        }
        if is_parked(hwnd) {
            let origin = ORIGIN.try_lock().ok().and_then(|origin| *origin);
            match origin {
                Some((parked, x, y)) if parked == hwnd => move_to(hwnd, x, y),
                // Never seen on screen: somewhere it can be seen.
                _ => move_to(hwnd, 200, 200),
            }
        }
        bring_forward(hwnd);
    }

    pub fn fill_and_open(hwnd: isize, paths: &[String]) -> bool {
        let dialog = HWND(hwnd as *mut _);
        if !is_open(hwnd) {
            return false;
        }
        let Some(edit) = file_name_edit(dialog) else {
            return false;
        };
        // One file goes in as its full path. Several are quoted names, which
        // the dialog reads against its current folder, so it is sent to that
        // folder first; a dialog that does not allow several takes the first.
        let text = match paths {
            [single] => single.clone(),
            many => {
                let folder = std::path::Path::new(&many[0])
                    .parent()
                    .map(|folder| folder.to_string_lossy().into_owned())
                    .unwrap_or_default();
                set_text(edit, &folder);
                press_open(dialog);
                // The dialog moves to the folder asynchronously.
                std::thread::sleep(std::time::Duration::from_millis(300));
                many.iter()
                    .filter_map(|path| std::path::Path::new(path).file_name())
                    .map(|name| format!("\"{}\"", name.to_string_lossy()))
                    .collect::<Vec<_>>()
                    .join(" ")
            }
        };
        set_text(edit, &text);
        press_open(dialog);
        true
    }

    fn file_name_edit(dialog: HWND) -> Option<HWND> {
        if let Ok(combo) = unsafe { GetDlgItem(Some(dialog), FILE_NAME_COMBO) } {
            // The older dialog: the box is the combo's own child.
            if let Ok(edit) =
                unsafe { FindWindowExW(Some(combo), None, w!("Edit"), PCWSTR::null()) }
            {
                return Some(edit);
            }
            // The one Windows 10 and 11 show (Firefox and Zen use it): 1148
            // is a ComboBoxEx32, with a ComboBox inside and the box inside
            // that. Looked for one level up only, it was never found, and a
            // double click sent nothing (07/11/2026).
            if let Ok(inner) =
                unsafe { FindWindowExW(Some(combo), None, w!("ComboBox"), PCWSTR::null()) }
                && let Ok(edit) =
                    unsafe { FindWindowExW(Some(inner), None, w!("Edit"), PCWSTR::null()) }
            {
                return Some(edit);
            }
        }
        unsafe { GetDlgItem(Some(dialog), FILE_NAME_EDIT) }.ok()
    }

    fn set_text(edit: HWND, text: &str) {
        let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        unsafe {
            SendMessageW(
                edit,
                WM_SETTEXT,
                Some(WPARAM(0)),
                Some(LPARAM(wide.as_ptr() as isize)),
            );
        }
    }

    // The Open button's own click, sent as the command it would send, so it
    // works without the dialog having focus.
    fn press_open(dialog: HWND) {
        let button = unsafe { GetDlgItem(Some(dialog), IDOK.0) }.ok();
        let command = (IDOK.0 as usize & 0xFFFF) | ((BN_CLICKED as usize) << 16);
        unsafe {
            SendMessageW(
                dialog,
                WM_COMMAND,
                Some(WPARAM(command)),
                Some(LPARAM(button.map(|button| button.0 as isize).unwrap_or(0))),
            );
        }
    }
}

#[cfg(not(windows))]
mod platform {
    use super::Dialog;

    pub fn start_hook(_sender: async_channel::Sender<Dialog>) -> bool {
        false
    }
    pub fn is_open(_hwnd: isize) -> bool {
        false
    }
    pub fn hide(_hwnd: isize) {}
    pub fn file_types(_hwnd: isize) -> (Vec<String>, usize) {
        (Vec::new(), 0)
    }
    pub fn foreground_file_dialog() -> Option<isize> {
        None
    }
    pub fn bring_forward(_hwnd: isize) {}
    pub fn explorer_folder(_hwnd: isize) -> Option<String> {
        None
    }
    pub fn is_parked(_hwnd: isize) -> bool {
        true
    }
    pub fn show(_hwnd: isize) {}
    pub fn fill_and_open(_hwnd: isize, _paths: &[String]) -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::{matches_any, parse_patterns, parse_typed_patterns};

    #[test]
    fn file_types_list_their_patterns() {
        assert_eq!(parse_patterns("Todos os arquivos (*.*)"), None);
        assert_eq!(
            parse_patterns("Imagens (*.png;*.JPG)"),
            Some(vec!["*.png".to_string(), "*.jpg".to_string()])
        );
        assert_eq!(
            parse_patterns("Image Files (*.png, *.gif)"),
            Some(vec!["*.png".into(), "*.gif".into()])
        );
        assert_eq!(parse_patterns("Arquivos personalizados"), None);
        assert_eq!(
            parse_typed_patterns("*.mp4 *.mov"),
            Some(vec!["*.mp4".into(), "*.mov".into()])
        );
    }

    #[test]
    fn names_match_patterns_without_case() {
        let images = vec!["*.png".to_string(), "*.jpg".to_string()];
        assert!(matches_any(&images, "Foto.PNG"));
        assert!(!matches_any(&images, "video.mp4"));
        assert!(matches_any(&["video??.mp4".to_string()], "video01.mp4"));
        assert!(!matches_any(&["*.png".to_string()], "png"));
    }
}
