//! Videos Windows cannot play, previewed through a light copy FFmpeg makes.
//!
//! Media Foundation, which the preview's player is, opens H.264 and HEVC in
//! a .mov, but not what editors put in one: ProRes, QuickTime Animation
//! (qtrle, with transparency), Motion JPEG, DNxHD. Measured on the owner's
//! own .mov files, every one was of these. When the player cannot show a
//! video, FFmpeg — if the machine has it — writes an H.264 copy no larger
//! than the preview needs, kept in the cache by the file's path, size and
//! date, and the player plays that instead, with its seek, volume and zoom.
//!
//! A video with transparency (Animation and ProRes 4444 usually have it)
//! is drawn over a checkerboard, as editors show it: flattened, the clear
//! parts came out black, and a title that starts transparent showed a
//! black screen.
//!
//! FFmpeg is looked for beside Prime Files and on the PATH. Without it the
//! preview says what is missing rather than a bare "cannot play".

use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::OnceLock;

/// How the copies are made; see `proxy_path`.
const COPY_VERSION: u32 = 2;

/// Widest copy made: the preview's frames are scaled to 1280×720 anyway.
const MAX_WIDTH: u32 = 1280;
/// Past this, the oldest copies are deleted after a new one is made.
const CACHE_LIMIT: u64 = 2 * 1024 * 1024 * 1024;

// No console window flashing for each conversion.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// The FFmpeg to use, if there is one: beside the app, then on the PATH.
/// Looked for once per run.
pub(crate) fn ffmpeg() -> Option<&'static Path> {
    static FOUND: OnceLock<Option<PathBuf>> = OnceLock::new();
    FOUND
        .get_or_init(|| {
            let name = if cfg!(windows) {
                "ffmpeg.exe"
            } else {
                "ffmpeg"
            };
            let beside = std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(|dir| dir.join(name)));
            beside
                .into_iter()
                .chain(on_path(name))
                .find(|candidate| candidate.is_file())
        })
        .as_deref()
}

fn on_path(name: &str) -> Vec<PathBuf> {
    std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths)
                .map(|dir| dir.join(name))
                .collect()
        })
        .unwrap_or_default()
}

/// Where the copy of `path` is kept: by its path, size and date, so an
/// edited video gets a new copy.
#[allow(clippy::disallowed_methods)]
pub(crate) fn proxy_path(path: &str) -> PathBuf {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    // Bumped when the copies are made differently, so old ones are not used.
    COPY_VERSION.hash(&mut hasher);
    path.to_lowercase().hash(&mut hasher);
    if let Ok(metadata) = std::fs::metadata(path) {
        metadata.len().hash(&mut hasher);
        if let Ok(modified) = metadata.modified() {
            modified.hash(&mut hasher);
        }
    }
    cache_dir().join(format!("{:016x}.mp4", hasher.finish()))
}

fn cache_dir() -> PathBuf {
    nohrs_core::config::paths::cache_dir().join("video-proxies")
}

/// The copy of `path`, made now if it is not in the cache. `progress` hears
/// 0..=1 as FFmpeg goes, or `None` when the length is not known; the work
/// stops (and the half-made copy goes) as soon as `cancelled` says so.
#[allow(clippy::disallowed_methods)]
pub(crate) fn make(
    path: &str,
    cancelled: &dyn Fn() -> bool,
    progress: &mut dyn FnMut(Option<f32>),
) -> Result<PathBuf, String> {
    let ffmpeg = ffmpeg().ok_or_else(|| {
        nohrs_core::t!(
            "O Windows não abre este formato de vídeo. Instale o FFmpeg para ver a prévia."
        )
        .to_string()
    })?;
    let out = proxy_path(path);
    if out.is_file() {
        return Ok(out);
    }
    std::fs::create_dir_all(cache_dir()).map_err(|error| error.to_string())?;
    let part = out.with_extension("part.mp4");
    let probe = probe(ffmpeg, path);
    let duration = probe.duration;
    progress(duration.map(|_| 0.0));

    let scale = format!("scale='trunc(min({MAX_WIDTH},iw)/2)*2':-2");
    let filter = if probe.alpha {
        // The clear parts over a checkerboard of 12-pixel squares.
        format!(
            "[0:v]{scale},format=yuva420p,split[fg][ref];\
             [ref]geq=lum='if(eq(mod(floor(X/12)+floor(Y/12),2),0),92,70)':cb=128:cr=128:a=255,format=yuv420p[bg];\
             [bg][fg]overlay=format=auto,format=yuv420p[v]"
        )
    } else {
        format!("[0:v]{scale},format=yuv420p[v]")
    };
    let mut command = Command::new(ffmpeg);
    command
        .args(["-nostdin", "-v", "error", "-y", "-i"])
        .arg(path)
        .args(["-filter_complex", &filter, "-map", "[v]", "-map", "0:a:0?"])
        .args(["-c:v", "libx264", "-preset", "ultrafast", "-crf", "22"])
        .args(["-c:a", "aac", "-b:a", "160k", "-movflags", "+faststart"])
        .args(["-progress", "pipe:1", "-nostats"])
        .arg(&part)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        // Errors are read after the end; `-v error` keeps them few, so the
        // pipe does not fill and stall FFmpeg meanwhile.
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    let stdout = child.stdout.take().ok_or("no output")?;
    for line in BufReader::new(stdout).lines() {
        if cancelled() {
            child.kill().ok();
            child.wait().ok();
            std::fs::remove_file(&part).ok();
            return Err(String::new());
        }
        let Ok(line) = line else { break };
        if let (Some(total), Some(done)) = (duration, line.strip_prefix("out_time_us=")) {
            if let Ok(micros) = done.trim().parse::<f64>() {
                progress(Some((micros / 1_000_000.0 / total).clamp(0.0, 1.0) as f32));
            }
        }
    }
    let output = child
        .wait_with_output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        std::fs::remove_file(&part).ok();
        let said = String::from_utf8_lossy(&output.stderr);
        let first = said
            .lines()
            .find(|line| !line.trim().is_empty())
            .unwrap_or("")
            .trim()
            .to_string();
        tracing::warn!("ffmpeg could not convert {path}: {said}");
        return Err(nohrs_core::tf!(
            "O FFmpeg não conseguiu converter este vídeo: {error}",
            error = first
        ));
    }
    std::fs::rename(&part, &out).map_err(|error| error.to_string())?;
    prune(&out);
    Ok(out)
}

/// What ffprobe says of a video.
#[derive(Default)]
struct Probe {
    /// Seconds, for the progress.
    duration: Option<f64>,
    /// The picture has an alpha channel.
    alpha: bool,
}

// Asks the ffprobe beside the FFmpeg; without one, no progress and no
// checkerboard.
fn probe(ffmpeg: &Path, path: &str) -> Probe {
    let ffprobe = ffmpeg.with_file_name("ffprobe.exe");
    if !ffprobe.is_file() {
        return Probe::default();
    }
    let mut command = Command::new(ffprobe);
    command
        .args(["-v", "error", "-select_streams", "v:0"])
        .args([
            "-show_entries",
            "stream=pix_fmt:format=duration",
            "-of",
            "default=nw=1",
        ])
        .arg(path)
        .stdin(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    let Ok(output) = command.output() else {
        return Probe::default();
    };
    parse_probe(&String::from_utf8_lossy(&output.stdout))
}

fn parse_probe(text: &str) -> Probe {
    let mut probe = Probe::default();
    for line in text.lines() {
        if let Some(seconds) = line.strip_prefix("duration=") {
            probe.duration = seconds
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|seconds| *seconds > 0.0);
        } else if let Some(format) = line.strip_prefix("pix_fmt=") {
            probe.alpha = has_alpha(format.trim());
        }
    }
    probe
}

// FFmpeg's pixel formats that carry alpha.
fn has_alpha(format: &str) -> bool {
    format.starts_with("yuva")
        || format.starts_with("gbrap")
        || format.starts_with("ya")
        || ["argb", "rgba", "abgr", "bgra", "rgba64", "bgra64"]
            .iter()
            .any(|name| format.starts_with(name))
}

// Keeps the copies under `CACHE_LIMIT`, the least recently made going first;
// `keep` (the one just made) always stays.
#[allow(clippy::disallowed_methods)]
fn prune(keep: &Path) {
    let Ok(read) = std::fs::read_dir(cache_dir()) else {
        return;
    };
    let mut copies: Vec<(PathBuf, u64, std::time::SystemTime)> = read
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let metadata = entry.metadata().ok()?;
            Some((
                entry.path(),
                metadata.len(),
                metadata.modified().unwrap_or(std::time::UNIX_EPOCH),
            ))
        })
        .collect();
    let mut total: u64 = copies.iter().map(|(_, size, _)| size).sum();
    copies.sort_by_key(|(_, _, modified)| *modified);
    for (path, size, _) in copies {
        if total <= CACHE_LIMIT {
            break;
        }
        if path != keep && std::fs::remove_file(&path).is_ok() {
            total -= size;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::proxy_path;

    #[test]
    fn transparency_is_read_from_the_pixel_format() {
        let probe = super::parse_probe("pix_fmt=yuva444p12le\nduration=7.007000\n");
        assert!(probe.alpha);
        assert_eq!(probe.duration, Some(7.007));
        assert!(super::parse_probe("pix_fmt=argb\n").alpha);
        assert!(!super::parse_probe("pix_fmt=yuvj422p\nduration=N/A\n").alpha);
        assert_eq!(super::parse_probe("duration=N/A\n").duration, None);
    }

    #[test]
    fn the_copy_is_kept_by_path_without_case() {
        assert_eq!(proxy_path(r"C:\x\Clip.MOV"), proxy_path(r"c:\x\clip.mov"));
        assert_ne!(proxy_path(r"C:\x\a.mov"), proxy_path(r"C:\x\b.mov"));
        assert_eq!(
            proxy_path(r"C:\x\a.mov")
                .extension()
                .and_then(|e| e.to_str()),
            Some("mp4")
        );
    }
}
