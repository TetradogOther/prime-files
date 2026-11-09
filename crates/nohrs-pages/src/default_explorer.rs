//! Making Nohrs the folder handler of the current Windows user, from the
//! settings page. The same registry entries `integrations\windows\tornar-padrao.bat`
//! writes, plus Win+E and the taskbar's Explorer button.
//!
//! All of it lives under `HKCU\Software\Classes`, so it needs no administrator
//! and touches no other account; turning it off deletes only what was added
//! here. Written through `reg.exe` rather than the registry API so each entry
//! reads exactly like the line in the `.bat` that a person can check by hand.
//!
//! On Linux the same thing is done the way the desktop does it: a `.desktop`
//! file in the user's own applications folder, and the `inode/directory` kind
//! pointed at it in `mimeapps.list`.

#[cfg(windows)]
use nohrs_core::t;
use std::io;
use std::path::Path;

#[cfg(windows)]
use std::process::Command;

/// The name of the program's `.desktop` file, without its extension.
pub const DESKTOP_ID: &str = "prime-files";

/// The kind of file the desktop asks for when a folder is opened.
const FOLDER_MIME: &str = "inode/directory";

// The verb added under `Directory\shell` and `Drive\shell`; the key's default
// value names it, which makes it the double-click action.
const VERB: &str = "nohrs";
const FOLDER_CLASSES: [&str; 2] = ["Directory", "Drive"];
// What Win+E and the taskbar's Explorer button launch ("This PC" in a new
// window). A `command` with an empty `DelegateExecute` under it replaces the
// COM handler with a plain program for this user, which is how replacement
// file managers take those two over.
const NEW_WINDOW_KEY: &str =
    r"HKCU\Software\Classes\CLSID\{52205fd8-5dfb-447d-801a-d0b52f2e83e1}\shell\opennewwindow";
// The same object's default verb. Win+E asks for `opennewwindow`, but the
// File Explorer pinned to the taskbar (and the Start menu's) is a shortcut to
// the object itself, which runs `open`: with only the first taken over, the
// taskbar button kept opening Explorer (Prime Files issue #4).
const OPEN_KEY: &str = r"HKCU\Software\Classes\CLSID\{52205fd8-5dfb-447d-801a-d0b52f2e83e1}\shell\open";

#[cfg(windows)]
fn reg(args: &[&str]) -> io::Result<std::process::Output> {
    let mut command = Command::new("reg.exe");
    command.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // No console flashing up over the window for each call.
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    command.output()
}

#[cfg(windows)]
fn reg_ok(args: &[&str]) -> io::Result<()> {
    let output = reg(args)?;
    if output.status.success() {
        Ok(())
    } else {
        Err(io::Error::other(String::from_utf8_lossy(&output.stderr).trim().to_string()))
    }
}

/// Whether folders currently open in Nohrs for this user.
#[cfg(windows)]
pub fn is_default() -> bool {
    reg(&["query", r"HKCU\Software\Classes\Directory\shell", "/ve"])
        .map(|output| {
            output.status.success()
                && String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .any(|line| line.contains("REG_SZ") && line.trim_end().ends_with(VERB))
        })
        .unwrap_or(false)
}

/// Makes `exe` open folders, drives, Win+E and the taskbar's Explorer button.
#[cfg(windows)]
pub fn enable(exe: &Path) -> io::Result<()> {
    let exe = exe.to_string_lossy();
    let quoted = format!("\"{exe}\"");
    for class in FOLDER_CLASSES {
        let shell = format!(r"HKCU\Software\Classes\{class}\shell");
        let verb = format!(r"{shell}\{VERB}");
        reg_ok(&["add", &verb, "/ve", "/d", t!("Abrir no Prime Files"), "/f"])?;
        reg_ok(&["add", &verb, "/v", "Icon", "/d", &quoted, "/f"])?;
        reg_ok(&["add", &format!(r"{verb}\command"), "/ve", "/d", &format!("{quoted} \"%1\""), "/f"])?;
        reg_ok(&["add", &shell, "/ve", "/d", VERB, "/f"])?;
    }
    launcher_verbs(&quoted)
}

// Without an argument Nohrs brings its open window forward, or opens one.
#[cfg(windows)]
fn launcher_verbs(quoted: &str) -> io::Result<()> {
    for key in [NEW_WINDOW_KEY, OPEN_KEY] {
        let command = format!(r"{key}\command");
        reg_ok(&["add", &command, "/ve", "/d", quoted, "/f"])?;
        reg_ok(&["add", &command, "/v", "DelegateExecute", "/d", "", "/f"])?;
    }
    Ok(())
}

/// For a copy made the default before the taskbar's verb was taken over
/// too: gives `open` the program `opennewwindow` already names. Never points
/// either at the running executable, which may be a test copy; does nothing
/// while Prime Files is not the default or `open` already has a command.
#[cfg(windows)]
pub fn repair() -> io::Result<()> {
    // Only the installed copy: a build run from the source tree, or a test
    // copy, must not change what the person's taskbar opens.
    let installed = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|folder| folder.join("unins000.exe").is_file()))
        .unwrap_or(false);
    if !installed || !is_default() {
        return Ok(());
    }
    let command_of = |key: &str| -> Option<String> {
        let output = reg(&["query", &format!(r"{key}\command"), "/ve"]).ok()?;
        String::from_utf8_lossy(&output.stdout)
            .lines()
            .find_map(|line| line.split_once("REG_SZ").map(|(_, value)| value.trim().to_string()))
            .filter(|value| !value.is_empty())
    };
    if command_of(OPEN_KEY).is_some() || !is_ours(&format!(r"{NEW_WINDOW_KEY}\command")) {
        return Ok(());
    }
    let Some(program) = command_of(NEW_WINDOW_KEY) else {
        return Ok(());
    };
    let command = format!(r"{OPEN_KEY}\command");
    reg_ok(&["add", &command, "/ve", "/d", &program, "/f"])?;
    reg_ok(&["add", &command, "/v", "DelegateExecute", "/d", "", "/f"])
}

/// Gives folders back to Explorer, removing only what [`enable`] added.
#[cfg(windows)]
pub fn disable() -> io::Result<()> {
    for class in FOLDER_CLASSES {
        let shell = format!(r"HKCU\Software\Classes\{class}\shell");
        // Not `reg_ok`: a key that is not there fails the delete, and is
        // already undone.
        reg(&["delete", &format!(r"{shell}\{VERB}"), "/f"])?;
        if is_ours(&shell) {
            reg_ok(&["delete", &shell, "/ve", "/f"])?;
        }
    }
    for key in [NEW_WINDOW_KEY, OPEN_KEY] {
        if is_ours(&format!(r"{key}\command")) {
            reg_ok(&["delete", key, "/f"])?;
        }
    }
    Ok(())
}

// Whether the default value of `key` points at this app, so a value some
// other program set is left alone. The verb's name says "nohrs"; a command
// names the executable, which the installer calls PrimeFiles.exe.
#[cfg(windows)]
fn is_ours(key: &str) -> bool {
    let exe_name = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.file_name().map(|name| name.to_string_lossy().to_ascii_lowercase()));
    reg(&["query", key, "/ve"])
        .map(|output| {
            String::from_utf8_lossy(&output.stdout).lines().any(|line| {
                let line = line.to_ascii_lowercase();
                line.contains("reg_sz")
                    && (line.contains("nohrs") || exe_name.as_deref().is_some_and(|name| line.contains(name)))
            })
        })
        .unwrap_or(false)
}

// Where Windows starts programs at sign-in for this user, and the value name.
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "Prime Files";

/// Whether Prime Files starts, minimized, when this user signs in.
#[cfg(windows)]
pub fn starts_with_windows() -> bool {
    reg(&["query", RUN_KEY, "/v", RUN_VALUE]).is_ok_and(|output| output.status.success())
}

/// Makes `exe` start minimized at sign-in, or stops it. Started that way it
/// is already running when Premiere or a browser asks for a folder, so the
/// file opens here at once instead of in Explorer.
#[cfg(windows)]
pub fn set_starts_with_windows(exe: &Path, enable: bool) -> io::Result<()> {
    if enable {
        let command = format!("\"{}\" --background", exe.display());
        reg_ok(&["add", RUN_KEY, "/v", RUN_VALUE, "/t", "REG_SZ", "/d", &command, "/f"])
    } else if starts_with_windows() {
        reg_ok(&["delete", RUN_KEY, "/v", RUN_VALUE, "/f"])
    } else {
        Ok(())
    }
}

/// Whether Prime Files starts, minimized, when this user signs in.
#[cfg(not(windows))]
pub fn starts_with_windows() -> bool {
    linux::autostart_file().is_some_and(|path| path.is_file())
}

/// Makes `exe` start minimized at sign-in, or stops it. Started that way it
/// is already running when Premiere or a browser asks for a folder, so the
/// file opens here at once instead of in Explorer.
#[cfg(not(windows))]
pub fn set_starts_with_windows(exe: &Path, enable: bool) -> io::Result<()> {
    linux::set_autostart(exe, enable)
}

/// Whether folders currently open in Nohrs for this user.
#[cfg(not(windows))]
pub fn is_default() -> bool {
    linux::is_default()
}

/// Makes `exe` open folders, drives, Win+E and the taskbar's Explorer button.
#[cfg(not(windows))]
pub fn enable(exe: &Path) -> io::Result<()> {
    linux::enable(exe)
}

/// For a copy made the default before the taskbar's verb was taken over
/// too: does nothing on Linux, where the desktop keeps no such second verb.
#[cfg(not(windows))]
pub fn repair() -> io::Result<()> {
    Ok(())
}

/// Gives folders back to Explorer, removing only what [`enable`] added.
#[cfg(not(windows))]
pub fn disable() -> io::Result<()> {
    linux::disable()
}

/// The desktop's own way of making a program the one that opens folders: a
/// `.desktop` file for it, and the `inode/directory` kind pointed at that
/// file in `mimeapps.list`.
#[cfg(not(windows))]
mod linux {
    use super::{DESKTOP_ID, FOLDER_MIME};
    use std::io;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    /// The file the desktop reads the program's own entries from.
    fn desktop_file() -> PathBuf {
        crate::linux::data_home().join("applications").join(format!("{DESKTOP_ID}.desktop"))
    }

    /// The file the desktop reads the user's own choices from.
    fn mimeapps_file() -> PathBuf {
        crate::linux::config_home().join("mimeapps.list")
    }

    /// Whether the desktop opens folders in this program.
    pub fn is_default() -> bool {
        let output = Command::new("xdg-mime").arg("query").arg("default").arg(FOLDER_MIME).output();
        match output {
            Ok(output) if output.status.success() => {
                String::from_utf8_lossy(&output.stdout).trim() == format!("{DESKTOP_ID}.desktop")
            }
            // Without the command, the user's own file is the answer.
            _ => mimeapps_default().is_some_and(|entry| entry == format!("{DESKTOP_ID}.desktop")),
        }
    }

    /// The program the user's own file names for folders.
    fn mimeapps_default() -> Option<String> {
        let text = std::fs::read_to_string(mimeapps_file()).ok()?;
        let mut in_section = false;
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                in_section = line == "[Default Applications]";
                continue;
            }
            if !in_section {
                continue;
            }
            if let Some((kind, value)) = line.split_once('=')
                && kind.trim() == FOLDER_MIME
            {
                let value = value.trim();
                if !value.is_empty() {
                    return Some(value.to_string());
                }
            }
        }
        None
    }

    /// Makes `exe` open folders: the program's own `.desktop` file, and the
    /// desktop's list pointing folders at it.
    pub fn enable(exe: &Path) -> io::Result<()> {
        let path = desktop_file();
        let directory = path.parent().unwrap_or(Path::new("/"));
        std::fs::create_dir_all(directory)?;
        let icon = icon_path();
        let text = format!(
            "[Desktop Entry]\nType=Application\nName=Prime Files\nComment=Explorador de arquivos\nExec={} %f\nIcon={}\nTerminal=false\nCategories=System;FileManager;FileTools;\nMimeType={};\n",
            exe.display(),
            icon,
            FOLDER_MIME
        );
        std::fs::write(&path, text)?;

        // The desktop's own command writes the list in the file it keeps.
        let written = Command::new("xdg-mime")
            .arg("default")
            .arg(format!("{DESKTOP_ID}.desktop"))
            .arg(FOLDER_MIME)
            .status();
        if written.as_ref().is_ok_and(|status| !status.success()) {
            return Err(io::Error::other("the desktop refused the change"));
        }
        // The program is started with a folder as its argument; the desktop
        // gives it the file's URI, which it does not read.
        Ok(())
    }

    /// The icon the program's own file points at.
    fn icon_path() -> String {
        if let Ok(data) = std::env::var("XDG_DATA_HOME") {
            return format!("{data}/icons/hicolor/256x256/apps/{DESKTOP_ID}.png");
        }
        let home = dirs::home_dir().unwrap_or_default();
        format!("{}/.local/share/icons/hicolor/256x256/apps/{DESKTOP_ID}.png", home.display())
    }

    /// Gives folders back to the desktop, removing only what [`enable`]
    /// added: the program's own file, and the name it had in the user's
    /// list.
    pub fn disable() -> io::Result<()> {
        let path = desktop_file();
        if path.is_file() {
            std::fs::remove_file(&path)?;
        }
        let list = mimeapps_file();
        if let Ok(text) = std::fs::read_to_string(&list) {
            let kept = text
                .lines()
                .filter(|line| !line.trim_start().starts_with(&format!("{FOLDER_MIME}=")) || !line.contains(DESKTOP_ID))
                .collect::<Vec<_>>()
                .join("\n");
            std::fs::write(&list, kept)?;
        }
        Ok(())
    }

    /// Where the desktop keeps the programs that start at sign-in.
    pub(super) fn autostart_file() -> Option<PathBuf> {
        Some(crate::linux::config_home().join("autostart").join(format!("{DESKTOP_ID}.desktop")))
    }

    /// Starts `exe` at sign-in, or stops it.
    pub fn set_autostart(exe: &Path, enable: bool) -> io::Result<()> {
        let path = match autostart_file() {
            Some(path) => path,
            None => return Err(io::Error::other("no autostart folder")),
        };
        if !enable {
            if path.is_file() {
                std::fs::remove_file(&path)?;
            }
            return Ok(());
        }
        if let Some(directory) = path.parent() {
            std::fs::create_dir_all(directory)?;
        }
        let text = format!(
            "[Desktop Entry]\nType=Application\nName=Prime Files\nExec={} --background\nIcon=prime-files\nTerminal=false\nX-GNOME-Autostart-enabled=true\nNoDisplay=false\n",
            exe.display()
        );
        std::fs::write(&path, text)
    }
}
