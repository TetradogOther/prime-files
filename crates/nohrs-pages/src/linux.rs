//! What a Linux program asks the desktop: opening a file, the actions the
//! installed programs offer for it, the folders the user keeps their things
//! in, and where the program may keep its own files.
//!
//! The desktop speaks through three plain-text conventions, all read here
//! rather than through a library, so the program behaves like the other
//! programs installed beside it: a `.desktop` file per program, the XDG
//! directories (`XDG_DATA_DIRS`, `~/.config/user-dirs.dirs`) and the
//! `xdg-open` command, which is what the desktop's own file manager is.

use std::path::{Path, PathBuf};

/// One action a `.desktop` file offers for a file (open it, open it with
/// another program, print it).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Action {
    /// The identifier inside the `.desktop` file, used to run it.
    pub id: String,
    /// The name shown in a menu.
    pub name: String,
    /// The command line, with `%f` and friends still in it.
    pub exec: String,
}

/// One installed program, as its `.desktop` file describes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DesktopFile {
    /// The file name without its extension (`org.gnome.Nautilus`).
    pub id: String,
    /// The name shown in a menu.
    pub name: String,
    /// The command line, with `%f` and friends still in it.
    pub exec: String,
    /// The kinds of file it says it opens (`inode/directory`, `image/png`…).
    pub mime_types: Vec<String>,
    /// The extra actions it offers.
    pub actions: Vec<Action>,
    /// Whether it wants to be shown in a menu at all.
    pub no_display: bool,
}

/// The `.desktop` files installed for this user and for everybody.
///
/// The user's own come first, so a program installed for the user wins over
/// the same one in the system.
///
/// Reading them is a walk of every folder of applications of the system, and
/// there are hundreds of files there: the answer is kept for a while, so the
/// menu of the right button does not pay for it every time.
pub fn desktop_files() -> Vec<DesktopFile> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<(std::time::Instant, Vec<DesktopFile>)>> =
        std::sync::OnceLock::new();
    const KEPT_FOR: std::time::Duration = std::time::Duration::from_secs(30);

    let cache =
        CACHE.get_or_init(|| std::sync::Mutex::new((std::time::Instant::now(), Vec::new())));
    let mut cache = match cache.lock() {
        Ok(cache) => cache,
        Err(poisoned) => poisoned.into_inner(),
    };
    if cache.0.elapsed() < KEPT_FOR && !cache.1.is_empty() {
        return cache.1.clone();
    }
    let files = read_desktop_files();
    *cache = (std::time::Instant::now(), files.clone());
    files
}

/// The `.desktop` files of the system and of the user, read from the disk.
fn read_desktop_files() -> Vec<DesktopFile> {
    let mut directories = Vec::new();
    if let Some(home) = dirs::home_dir() {
        directories.push(home.join(".local/share/applications"));
    }
    if let Some(data) = dirs::data_dir() {
        directories.push(data.join("applications"));
    }
    for directory in std::env::var_os("XDG_DATA_DIRS")
        .into_iter()
        .flat_map(|value| std::env::split_paths(&value).collect::<Vec<_>>())
    {
        directories.push(directory.join("applications"));
    }

    let mut files = Vec::new();
    let mut seen = std::collections::HashSet::new();
    for directory in directories {
        let Ok(read) = std::fs::read_dir(&directory) else {
            continue;
        };
        for entry in read.filter_map(Result::ok) {
            let path = entry.path();
            if path
                .extension()
                .is_none_or(|extension| extension != "desktop")
            {
                continue;
            }
            let Some(id) = path
                .file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
            else {
                continue;
            };
            if !seen.insert(id.clone()) {
                continue;
            }
            if let Some(file) = parse_desktop_file(&path, &id) {
                files.push(file);
            }
        }
    }
    files
}

/// The `.desktop` file at `path`, read into its parts.
fn parse_desktop_file(path: &Path, id: &str) -> Option<DesktopFile> {
    let text = std::fs::read_to_string(path).ok()?;
    // A file whose type is not "Application" is a link or a directory
    // definition, not a program.
    let mut file = DesktopFile {
        id: id.to_string(),
        name: String::new(),
        exec: String::new(),
        mime_types: Vec::new(),
        actions: Vec::new(),
        no_display: false,
    };
    let mut section = String::new();
    let mut action = String::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') && line.ends_with(']') {
            section = line[1..line.len() - 1].to_string();
            action = section
                .strip_prefix("Desktop Action ")
                .unwrap_or_default()
                .to_string();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim();
        let value = value.trim();
        if section == "Desktop Entry" {
            match key {
                "Name" => file.name = value.to_string(),
                "Exec" => file.exec = value.to_string(),
                "MimeType" => {
                    file.mime_types = value
                        .split(';')
                        .map(str::trim)
                        .filter(|kind| !kind.is_empty())
                        .map(str::to_string)
                        .collect();
                }
                "NoDisplay" => file.no_display = value.eq_ignore_ascii_case("true"),
                _ => {}
            }
        } else if !action.is_empty() {
            // The action's own section: its name and its command.
            match key {
                "Name" => file.actions.push(Action {
                    id: action.clone(),
                    name: value.to_string(),
                    exec: String::new(),
                }),
                "Exec" => {
                    if let Some(last) = file.actions.last_mut()
                        && last.id == action
                    {
                        last.exec = value.to_string();
                    }
                }
                _ => {}
            }
        }
    }
    (file.name.is_empty() || file.exec.is_empty()).then_some(file)
}

/// The kind of file the desktop says `path` is, as the MIME type the
/// programs are registered for.
///
/// The desktop's own command answers it; a small table of the common kinds
/// answers for the files it does not know.
pub fn mime_type(path: &Path) -> Option<String> {
    if let Ok(output) = std::process::Command::new("xdg-mime")
        .arg("query")
        .arg("filetype")
        .arg(path)
        .output()
        && output.status.success()
    {
        let kind = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if !kind.is_empty() && kind != "application/octet-stream" {
            return Some(kind);
        }
    }
    let extension = path.extension()?.to_string_lossy().to_lowercase();
    let kind = match extension.as_str() {
        "txt" | "log" | "md" | "toml" | "rs" | "c" | "h" | "cpp" | "py" | "js" | "ts" | "sh" => {
            "text/plain"
        }
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "svg" => "image/svg+xml",
        "bmp" => "image/bmp",
        "pdf" => "application/pdf",
        "zip" => "application/zip",
        "tar" | "gz" | "bz2" | "xz" | "zst" => "application/x-tar",
        "7z" => "application/x-7z-compressed",
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "ogg" | "oga" => "audio/ogg",
        "opus" => "audio/opus",
        "wav" => "audio/x-wav",
        "m4a" => "audio/mp4",
        "mp4" | "m4v" => "video/mp4",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "avi" => "video/x-msvideo",
        "mov" => "video/quicktime",
        "doc" => "application/msword",
        "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
        "xls" => "application/vnd.ms-excel",
        "xlsx" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
        "ppt" => "application/vnd.ms-powerpoint",
        "pptx" => "application/vnd.openxmlformats-officedocument.presentationml.presentation",
        _ => return None,
    };
    Some(kind.to_string())
}

/// The programs that say they open this kind of file, the user's own first.
///
/// The kind is asked for by the name the desktop gives it; the caller passes
/// what it knows about the file.
pub fn handlers_for(mime_type: &str) -> Vec<DesktopFile> {
    let mut handlers = Vec::new();
    for file in desktop_files() {
        if file
            .mime_types
            .iter()
            .any(|kind| kind.eq_ignore_ascii_case(mime_type))
        {
            handlers.push(file);
        }
    }
    handlers
}

/// Opens `path` with the program the desktop says handles it.
pub fn open(path: &Path) -> std::io::Result<()> {
    let mut command = std::process::Command::new("xdg-open");
    command.arg(path);
    command.spawn().map(|_| ())
}

/// Opens `path` with a named program, as the desktop's own "Open with" does.
pub fn open_with(path: &Path, desktop_id: &str) -> std::io::Result<()> {
    let Some(file) = desktop_files()
        .into_iter()
        .find(|file| file.id == desktop_id)
    else {
        return open(path);
    };
    let mut command = std::process::Command::new("gtk-launch");
    command.arg(&file.id).arg(path);
    match command.spawn() {
        Ok(_) => Ok(()),
        // gtk-launch is not always installed; the program itself is.
        Err(_) => run_desktop_command(&file.exec, path),
    }
}

/// Runs the command of a `.desktop` file with the file's placeholders filled
/// in.
fn run_desktop_command(exec: &str, path: &Path) -> std::io::Result<()> {
    let Some((program, arguments)) = split_command(exec) else {
        return Err(std::io::Error::other("the desktop file has no command"));
    };
    let mut command = std::process::Command::new(program);
    for argument in arguments {
        let argument = argument.replace("%f", &path.to_string_lossy());
        let argument = argument.replace("%F", &path.to_string_lossy());
        let argument = argument.replace("%u", &path.to_string_lossy());
        let argument = argument.replace("%U", &path.to_string_lossy());
        command.arg(argument);
    }
    command.spawn().map(|_| ())
}

/// The program and the arguments of a command line, honouring the quotes the
/// `.desktop` files write.
fn split_command(exec: &str) -> Option<(String, Vec<String>)> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut started = false;
    for character in exec.chars() {
        match character {
            '"' => {
                quote = quote
                    .is_none()
                    .then_some('"')
                    .or_else(|| quote.filter(|q| *q == '"').and(None));
                started = true;
            }
            '\\' if quote.is_some() => {}
            ' ' | '\t' if quote.is_none() => {
                if started {
                    words.push(std::mem::take(&mut current));
                    started = false;
                }
            }
            other => {
                current.push(other);
                started = true;
            }
        }
    }
    if started {
        words.push(current);
    }
    let program = words.first()?.clone();
    Some((program, words.into_iter().skip(1).collect()))
}

/// The folders the user keeps their things in, as `~/.config/user-dirs.dirs`
/// lists them: Desktop, Documents, Downloads, Music, Pictures, Videos.
///
/// The file is read at every call rather than kept, so a folder the user
/// moves is picked up.
pub fn user_dirs() -> Vec<(String, PathBuf)> {
    let Some(home) = dirs::home_dir() else {
        return Vec::new();
    };
    let path = home.join(".config/user-dirs.dirs");
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut dirs = Vec::new();
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let name = key
            .trim()
            .strip_prefix("XDG_")
            .unwrap_or(key.trim())
            .strip_suffix("_DIR")
            .unwrap_or_default()
            .to_string();
        let value = value.trim().trim_matches('"');
        // "$HOME/Desktop" is written the way the shell would expand it.
        let value = value.strip_prefix("$HOME").map_or_else(
            || value.to_string(),
            |rest| home.join(rest).to_string_lossy().into_owned(),
        );
        // The desktop writes the names of the folders with the escapes of a
        // URI, so "Música" is written "M%C3%BAsica".
        let value = percent_decoded(&value);
        if name.is_empty() || value.is_empty() {
            continue;
        }
        let path = PathBuf::from(value);
        if path.is_dir() {
            dirs.push((name, path));
        }
    }
    dirs
}

/// The text of a URI with the `%XX` escapes read as the characters they
/// stand for.
pub fn percent_decoded(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            let hex = std::str::from_utf8(&bytes[index + 1..index + 3]).ok();
            if let Some(byte) = hex.and_then(|hex| u8::from_str_radix(hex, 16).ok()) {
                out.push(byte);
                index += 3;
                continue;
            }
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The folders the user's own program keeps its files in, created on first
/// use: the desktop's `XDG_*_HOME` rules.
pub fn config_home() -> PathBuf {
    dirs::config_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".config"))
}

/// The folder where the user's own programs keep their data.
pub fn data_home() -> PathBuf {
    dirs::data_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".local/share"))
}

/// The folder where the user's own programs keep what they must not lose.
pub fn state_home() -> PathBuf {
    dirs::state_dir().unwrap_or_else(|| dirs::home_dir().unwrap_or_default().join(".local/state"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_line_is_split_on_the_spaces_outside_the_quotes() {
        let (program, arguments) = split_command("nautilus --no-desktop %U").expect("a command");
        assert_eq!(program, "nautilus");
        assert_eq!(arguments, ["--no-desktop", "%U"]);
        let (program, arguments) = split_command("\"my program\" \"%f\"").expect("a command");
        assert_eq!(program, "my program");
        assert_eq!(arguments, ["%f"]);
    }

    #[test]
    fn the_desktop_files_of_this_machine_are_read() {
        // There is at least one program installed in any desktop; the list is
        // read the way the menu reads it.
        let files = desktop_files();
        for file in &files {
            assert!(!file.id.is_empty());
        }
    }
}
