//! Remembers the volume Windows' Volume Mixer gives Prime Files (issue #2).
//!
//! Media Foundation's media engine, which plays the preview and the sound on
//! hover, opens its audio session with a new random id on every run, and
//! Windows keeps a mixer setting under the session's id: set Prime Files to
//! 30 % in the mixer, start it again, and it was back at 100 %. The Snipping
//! Tool, built on the same engine, does the same. So the app keeps the value
//! itself: a thread looks at this process's sessions now and then, writes the
//! level down when the person changes it, and gives every new session the
//! level written down.

use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::mpsc;
use std::time::Duration;

/// Starts watching (once per run) and looks again right away. Called where a
/// media engine is made: its session appears a moment later, and is given
/// the kept level within a tenth of a second instead of at the next look —
/// until 06/11/2026 a sound started at 100 % for up to the whole interval
/// (issue #17).
pub(crate) fn watch() {
    static NUDGE: OnceLock<Option<mpsc::Sender<()>>> = OnceLock::new();
    let nudge = NUDGE.get_or_init(|| {
        let (sender, receiver) = mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("audio-session".into())
            .spawn(move || platform::run(store_path(), receiver));
        match spawned {
            Ok(_) => Some(sender),
            Err(error) => {
                tracing::warn!("could not watch the audio session: {error}");
                None
            }
        }
    });
    if let Some(nudge) = nudge
        && nudge.send(()).is_err()
    {
        tracing::debug!("the audio session watcher is gone");
    }
}

const POLL: Duration = Duration::from_millis(1500);
/// How often to look while a new engine's session may be appearing.
const QUICK: Duration = Duration::from_millis(100);
/// How long after a nudge to keep looking that often.
const QUICK_FOR: Duration = Duration::from_secs(3);

fn store_path() -> PathBuf {
    nohrs_core::config::paths::data_dir().join("mixer-volume")
}

/// The mixer's level and mute as the file keeps them: `0.300 1`.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Level {
    volume: f32,
    muted: bool,
}

impl Level {
    fn parse(text: &str) -> Option<Self> {
        let mut parts = text.split_whitespace();
        let volume: f32 = parts.next()?.parse().ok()?;
        let muted = parts.next().is_some_and(|flag| flag == "1");
        (0.0..=1.0)
            .contains(&volume)
            .then_some(Self { volume, muted })
    }

    fn format(self) -> String {
        format!("{:.3} {}", self.volume, u8::from(self.muted))
    }

    // The mixer moves in whole percents; anything finer is rounding.
    fn differs(self, other: Self) -> bool {
        self.muted != other.muted || (self.volume - other.volume).abs() > 0.004
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod platform {
    //! Core Audio has no safe binding.

    use super::{Level, POLL, QUICK, QUICK_FOR};
    use std::collections::HashSet;
    use std::path::PathBuf;
    use std::sync::mpsc;
    use std::time::Instant;
    use windows::Win32::Media::Audio::{
        IAudioSessionControl2, IAudioSessionManager2, IMMDeviceEnumerator, ISimpleAudioVolume,
        MMDeviceEnumerator, eMultimedia, eRender,
    };
    use windows::Win32::System::Com::{
        CLSCTX_ALL, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
    };
    use windows::core::Interface;

    pub fn run(path: PathBuf, nudges: mpsc::Receiver<()>) {
        if let Err(error) = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.ok() {
            tracing::debug!("CoInitializeEx: {error}");
        }
        let mut saved = std::fs::read_to_string(&path)
            .ok()
            .as_deref()
            .and_then(Level::parse);
        let mut adopted: HashSet<String> = HashSet::new();
        let mut quick_until: Option<Instant> = None;
        loop {
            match own_sessions() {
                Ok(sessions) => {
                    for (id, volume) in sessions {
                        if adopted.insert(id) {
                            // A session this run had not seen: it starts at
                            // what the person last chose.
                            if let Some(level) = saved {
                                apply(&volume, level);
                            }
                            continue;
                        }
                        let Some(now) = read(&volume) else { continue };
                        if saved.is_none_or(|level| level.differs(now)) {
                            saved = Some(now);
                            if let Err(error) = write(&path, now) {
                                tracing::warn!("could not keep the mixer volume: {error}");
                            }
                        }
                    }
                }
                Err(error) => tracing::debug!("audio sessions: {error}"),
            }
            let wait = if quick_until.is_some_and(|until| Instant::now() < until) {
                QUICK
            } else {
                POLL
            };
            match nudges.recv_timeout(wait) {
                Ok(()) => quick_until = Some(Instant::now() + QUICK_FOR),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => return,
            }
        }
    }

    fn write(path: &PathBuf, level: Level) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, level.format())
    }

    // This process's sessions on the default output, by instance id.
    fn own_sessions() -> windows::core::Result<Vec<(String, ISimpleAudioVolume)>> {
        let pid = std::process::id();
        let mut found = Vec::new();
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
            let device = enumerator.GetDefaultAudioEndpoint(eRender, eMultimedia)?;
            let manager: IAudioSessionManager2 = device.Activate(CLSCTX_ALL, None)?;
            let sessions = manager.GetSessionEnumerator()?;
            for index in 0..sessions.GetCount()? {
                let Ok(control) = sessions.GetSession(index) else {
                    continue;
                };
                let Ok(control) = control.cast::<IAudioSessionControl2>() else {
                    continue;
                };
                if control.GetProcessId().ok() != Some(pid) {
                    continue;
                }
                let Ok(raw) = control.GetSessionInstanceIdentifier() else {
                    continue;
                };
                let id = raw.to_string().unwrap_or_default();
                CoTaskMemFree(Some(raw.0 as *const _));
                if let Ok(volume) = control.cast::<ISimpleAudioVolume>() {
                    found.push((id, volume));
                }
            }
        }
        Ok(found)
    }

    fn read(volume: &ISimpleAudioVolume) -> Option<Level> {
        unsafe {
            Some(Level {
                volume: volume.GetMasterVolume().ok()?,
                muted: volume.GetMute().ok()?.as_bool(),
            })
        }
    }

    fn apply(volume: &ISimpleAudioVolume, level: Level) {
        unsafe {
            if let Err(error) = volume.SetMasterVolume(level.volume, std::ptr::null()) {
                tracing::debug!("SetMasterVolume: {error}");
            }
            if let Err(error) = volume.SetMute(level.muted, std::ptr::null()) {
                tracing::debug!("SetMute: {error}");
            }
        }
    }
}

#[cfg(not(windows))]
mod platform {
    pub fn run(_path: std::path::PathBuf, _nudges: std::sync::mpsc::Receiver<()>) {}
}

#[cfg(test)]
mod tests {
    use super::Level;

    #[test]
    fn the_level_reads_back_what_it_wrote() {
        let level = Level {
            volume: 0.3,
            muted: true,
        };
        assert_eq!(Level::parse(&level.format()), Some(level));
        assert_eq!(Level::parse("1.5 0"), None);
        assert_eq!(Level::parse("lixo"), None);
        assert!(!level.differs(Level {
            volume: 0.302,
            muted: true
        }));
        assert!(level.differs(Level {
            volume: 0.3,
            muted: false
        }));
    }
}
