//! Video and audio in the preview pane, played by Windows' own Media
//! Foundation engine (what the Films & TV app uses), so every format Windows
//! plays plays here, hardware decoding included.
//!
//! The engine runs on a thread of its own in frame-server mode: it decodes,
//! and each new video frame is copied into a bitmap the size of the preview
//! and handed to gpui as an image. No window of the engine's is put over
//! the app, so menus and dialogs still draw on top of the video.

use gpui::{Context, RenderImage, Task};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;

use super::ExplorerPane;

// The preview's volume (an f64's bits) and mute, for the whole app.
static MEDIA_VOLUME: AtomicU64 = AtomicU64::new(0x3FF0_0000_0000_0000);
static MEDIA_MUTED: AtomicBool = AtomicBool::new(false);

/// Largest frame handed to the UI, in pixels. Frames are scaled down by the
/// engine to fit, so a 4K video costs what a 1080p one does.
const MAX_FRAME: (u32, u32) = (1280, 720);

/// What the preview's controls ask of the player.
pub(crate) enum Command {
    Play,
    Pause,
    /// Seconds from the start.
    Seek(f64),
    /// 0..=1.
    Volume(f64),
}

/// A video or audio file playing (or paused) in the preview.
pub(crate) struct MediaPreview {
    commands: mpsc::Sender<Command>,
    /// The frame on screen; `None` for audio and until the first arrives.
    pub frame: Option<Arc<RenderImage>>,
    /// Seconds.
    pub position: f64,
    /// Seconds; 0 until the file's length is known.
    pub duration: f64,
    pub playing: bool,
    pub has_video: bool,
    pub error: Option<String>,
    /// While a video Windows cannot play is converted for the preview
    /// (`video_proxy`): how far, 0..=1, or `Some(None)` with no length known.
    pub preparing: Option<Option<f32>>,
    /// Loudest sample of each `PEAK_BLOCKS_PER_SECOND`th of a second of an
    /// audio file, 0..=1, filled from the start as the file is read. Empty
    /// for video. An `Arc` so a frame's paint can hold it without a copy.
    pub peaks: Arc<Vec<f32>>,
    /// Loudest of `peaks`, which the waveform is scaled to.
    pub peak_max: f32,
    _task: Task<()>,
}

/// Resolution of the waveform: a peak per 10 ms, fine enough for a bar per
/// pixel of a 3-minute song across a wide pane.
pub(crate) const PEAK_BLOCKS_PER_SECOND: usize = 100;

enum Update {
    Frame(RenderImage),
    State {
        position: f64,
        duration: f64,
        playing: bool,
        has_video: bool,
    },
    /// The next stretch of `MediaPreview::peaks`.
    Peaks(Vec<f32>),
    /// Converting with FFmpeg (`Some`), or done with it (`None`).
    Preparing(Option<Option<f32>>),
    Failed(String),
}

fn extension(path: &str) -> String {
    std::path::Path::new(path)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase()
}

/// Whether `path` is a sound-only file, which the preview draws as a waveform.
pub(crate) fn is_audio(path: &str) -> bool {
    matches!(
        extension(path).as_str(),
        "mp3" | "wav" | "m4a" | "aac" | "flac" | "wma" | "ogg" | "opus"
    )
}

/// Whether the preview plays `path` as video or audio.
pub(crate) fn is_media(path: &str) -> bool {
    is_audio(path)
        || matches!(
            extension(path).as_str(),
            "mp4"
                | "m4v"
                | "mov"
                | "wmv"
                | "avi"
                | "mkv"
                | "webm"
                | "mpg"
                | "mpeg"
                | "3gp"
                | "ts"
                | "mts"
        )
}

/// The volume a player is told to play at.
fn effective_volume(volume: f64, muted: bool) -> f64 {
    if muted { 0.0 } else { volume.clamp(0.0, 1.0) }
}

impl MediaPreview {
    /// Sends `command` to the player.
    pub(crate) fn send(&self, command: Command) {
        // The player thread only ends when this preview is dropped.
        self.commands.send(command).ok();
    }
}

impl MediaPreview {
    /// Starts a player for `path`, paused on its first frame unless
    /// `autoplay`. `slot` finds it again in the view that keeps it, so what
    /// the player sends lands there; once the slot is empty the player ends.
    /// The pane's preview and the search box's (`palette`) both use it.
    pub(crate) fn start<V: 'static>(
        path: String,
        autoplay: bool,
        slot: fn(&mut V) -> Option<&mut MediaPreview>,
        cx: &mut Context<V>,
    ) -> Option<Self> {
        load_saved_volume();
        let (commands, command_receiver) = mpsc::channel();
        let (sender, receiver) = async_channel::unbounded();
        // Queued before the engine exists; the player applies it as it starts.
        let volume = f64::from_bits(MEDIA_VOLUME.load(Ordering::Relaxed));
        commands
            .send(Command::Volume(effective_volume(
                volume,
                MEDIA_MUTED.load(Ordering::Relaxed),
            )))
            .ok();
        if is_audio(&path) {
            // Its own reader on its own thread: playback starts at once and
            // the waveform fills in beside it. Both end with the preview,
            // when the receiver goes and their sends start failing.
            let (path, sender) = (path.clone(), sender.clone());
            let spawned = std::thread::Builder::new()
                .name("nohrs-waveform".into())
                .spawn(move || platform::waveform(&path, &sender));
            if let Err(error) = spawned {
                tracing::warn!("could not start reading the waveform: {error}");
            }
        }
        let spawned = std::thread::Builder::new()
            .name("nohrs-media".into())
            .spawn(move || platform::run(&path, &command_receiver, &sender));
        if let Err(error) = spawned {
            tracing::error!("could not start the media player: {error}");
            return None;
        }
        let task = cx.spawn(async move |this, cx| {
            while let Ok(update) = receiver.recv().await {
                let alive = this.update(cx, |view, cx| {
                    let Some(media) = slot(view) else {
                        return false;
                    };
                    match update {
                        Update::Frame(frame) => {
                            // The atlas keeps a copy of every image drawn;
                            // without this each frame would stay in it.
                            if let Some(old) = media.frame.replace(Arc::new(frame)) {
                                cx.drop_image(old, None);
                            }
                        }
                        Update::State {
                            position,
                            duration,
                            playing,
                            has_video,
                        } => {
                            media.position = position;
                            media.duration = duration;
                            media.playing = playing;
                            media.has_video = has_video;
                        }
                        Update::Peaks(stretch) => {
                            media.peak_max =
                                stretch.iter().fold(media.peak_max, |max, &p| max.max(p));
                            Arc::make_mut(&mut media.peaks).extend(stretch);
                        }
                        Update::Preparing(progress) => media.preparing = progress,
                        Update::Failed(error) => {
                            media.preparing = None;
                            media.error = Some(error);
                        }
                    }
                    cx.notify();
                    true
                });
                if !matches!(alive, Ok(true)) {
                    break;
                }
            }
        });
        let media = MediaPreview {
            commands,
            frame: None,
            position: 0.0,
            duration: 0.0,
            playing: autoplay,
            has_video: false,
            error: None,
            preparing: None,
            peaks: Arc::default(),
            peak_max: 0.0,
            _task: task,
        };
        // The engine takes Play before the file is open and starts once it
        // is, so autoplay needs no wait for the first state.
        if autoplay {
            media.send(Command::Play);
        }
        Some(media)
    }

    /// Plays or pauses.
    pub(crate) fn toggle(&mut self) {
        self.playing = !self.playing;
        self.send(if self.playing {
            Command::Play
        } else {
            Command::Pause
        });
    }

    /// Jumps to `fraction` (0..=1) of the length, once it is known.
    pub(crate) fn seek(&mut self, fraction: f64) {
        if self.duration > 0.0 {
            self.position = fraction.clamp(0.0, 1.0) * self.duration;
            self.send(Command::Seek(self.position));
        }
    }

    /// Stops the player and frees its last frame.
    pub(crate) fn close(self, cx: &mut gpui::App) {
        if let Some(frame) = self.frame {
            cx.drop_image(frame, None);
        }
    }
}

impl ExplorerPane {
    /// Opens `path` in the preview's player, paused on its first frame.
    pub(crate) fn open_media_preview(&mut self, path: String, cx: &mut Context<Self>) {
        let Some(media) = MediaPreview::start(
            path,
            self.preview_autoplay,
            |pane: &mut Self| pane.preview_media.as_mut(),
            cx,
        ) else {
            return;
        };
        self.preview_media = Some(media);
        cx.notify();
    }

    /// Stops the player and frees its last frame, when the preview moves on.
    pub(crate) fn close_media_preview(&mut self, cx: &mut gpui::App) {
        if let Some(media) = self.preview_media.take() {
            media.close(cx);
        }
    }

    /// Plays or pauses the previewed media.
    pub(crate) fn toggle_media(&mut self, cx: &mut Context<Self>) {
        if let Some(media) = self.preview_media.as_mut() {
            media.toggle();
            cx.notify();
        }
    }

    /// Jumps the previewed media to `fraction` (0..=1) of its length.
    pub(crate) fn seek_media(&mut self, fraction: f64, cx: &mut Context<Self>) {
        if let Some(media) = self.preview_media.as_mut()
            && media.duration > 0.0
        {
            media.seek(fraction);
            cx.notify();
        }
    }

    /// The preview's volume, 0..=1. One for every tab (issue #2): set in
    /// one tab, the next sound in another plays at it too.
    pub(crate) fn media_volume(&self) -> f64 {
        load_saved_volume();
        f64::from_bits(MEDIA_VOLUME.load(Ordering::Relaxed))
    }

    /// Whether the preview is muted, shared by every tab like the volume.
    pub(crate) fn media_muted(&self) -> bool {
        load_saved_volume();
        MEDIA_MUTED.load(Ordering::Relaxed)
    }

    /// Mutes or unmutes the previewed media.
    pub(crate) fn toggle_media_mute(&mut self, cx: &mut Context<Self>) {
        let muted = !self.media_muted();
        MEDIA_MUTED.store(muted, Ordering::Relaxed);
        // Unmuting a slider left at zero would still be silent.
        if !muted && self.media_volume() <= 0.0 {
            MEDIA_VOLUME.store(0.5_f64.to_bits(), Ordering::Relaxed);
        }
        self.send_media_volume(cx);
    }

    /// Sets the volume to `volume` (0..=1), unmuting.
    pub(crate) fn set_media_volume(&mut self, volume: f64, cx: &mut Context<Self>) {
        MEDIA_VOLUME.store(volume.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
        MEDIA_MUTED.store(false, Ordering::Relaxed);
        self.send_media_volume(cx);
    }

    fn send_media_volume(&mut self, cx: &mut Context<Self>) {
        save_volume(self.media_volume(), self.media_muted(), cx);
        if let Some(media) = self.preview_media.as_ref() {
            media.send(Command::Volume(effective_volume(
                self.media_volume(),
                self.media_muted(),
            )));
        }
        cx.notify();
    }
}

/// Heights (0..=1) of `bars` waveform bars spread over `duration` seconds,
/// each the loudest peak in its stretch. `None` for a stretch not read yet.
pub(crate) fn waveform_bars(
    peaks: &[f32],
    peak_max: f32,
    duration: f64,
    bars: usize,
) -> Vec<Option<f32>> {
    // Until the engine knows the length, what has been read fills the width.
    let blocks = if duration > 0.0 {
        (duration * PEAK_BLOCKS_PER_SECOND as f64).ceil() as usize
    } else {
        peaks.len()
    };
    if bars == 0 || blocks == 0 {
        return Vec::new();
    }
    let scale = if peak_max > 0.0 { 1.0 / peak_max } else { 0.0 };
    (0..bars)
        .map(|bar| {
            let start = bar * blocks / bars;
            let end = ((bar + 1) * blocks / bars).max(start + 1);
            let stretch = peaks
                .get(start..end.min(peaks.len()))
                .filter(|s| !s.is_empty())?;
            Some((stretch.iter().fold(0.0f32, |max, &p| max.max(p)) * scale).min(1.0))
        })
        .collect()
}

/// `1:05` or `1:02:05`.
pub(crate) fn clock(seconds: f64) -> String {
    let total = seconds.max(0.0) as u64;
    let (hours, minutes, seconds) = (total / 3600, total / 60 % 60, total % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod platform {
    //! The Media Foundation engine. `unsafe` is confined to this module: none
    //! of these interfaces has a safe binding.

    use super::{Command, MAX_FRAME, PEAK_BLOCKS_PER_SECOND, Update};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};
    use windows::Win32::Foundation::RECT;
    use windows::Win32::Graphics::Imaging::{
        CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICBitmap, IWICImagingFactory,
        WICBitmapCacheOnLoad, WICBitmapLockRead, WICRect,
    };
    use windows::Win32::Media::MediaFoundation::{
        CLSID_MFMediaEngineClassFactory, IMFAttributes, IMFMediaEngine, IMFMediaEngineClassFactory,
        IMFMediaEngineNotify, IMFMediaEngineNotify_Impl, IMFSample, MF_MEDIA_ENGINE_CALLBACK,
        MF_MEDIA_ENGINE_VIDEO_OUTPUT_FORMAT, MF_MT_AUDIO_NUM_CHANNELS,
        MF_MT_AUDIO_SAMPLES_PER_SECOND, MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE,
        MF_SOURCE_READER_ALL_STREAMS, MF_SOURCE_READER_FIRST_AUDIO_STREAM,
        MF_SOURCE_READERF_ENDOFSTREAM, MF_VERSION, MFARGB, MFAudioFormat_Float, MFCreateAttributes,
        MFCreateMediaType, MFCreateSourceReaderFromURL, MFMediaType_Audio, MFSTARTUP_FULL,
        MFShutdown, MFStartup,
    };
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
    };
    use windows::core::{BSTR, HSTRING, implement};

    // DXGI_FORMAT_B8G8R8A8_UNORM, the frame format asked of the engine: the
    // byte order gpui's images use, so frames need no swizzling.
    const B8G8R8A8: u32 = 87;

    // The engine insists on a listener; the player polls its state instead,
    // which keeps everything on this one thread.
    #[implement(IMFMediaEngineNotify)]
    struct Listener;

    impl IMFMediaEngineNotify_Impl for Listener_Impl {
        fn EventNotify(
            &self,
            _event: u32,
            _param1: usize,
            _param2: u32,
        ) -> windows::core::Result<()> {
            Ok(())
        }
    }

    pub fn run(
        path: &str,
        commands: &mpsc::Receiver<Command>,
        updates: &async_channel::Sender<Update>,
    ) {
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .ok();
        if let Err(error) = unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) } {
            updates
                .send_blocking(Update::Failed(error.message().to_string()))
                .ok();
            return;
        }
        let mut wanted = Wanted::default();
        let video = !super::is_audio(path);
        if let Err(error) = play(path, video, commands, updates, &mut wanted) {
            // A video Windows cannot decode (ProRes, Animation...): a copy
            // FFmpeg makes plays instead, with what was asked meanwhile.
            if error.code() == CLOSED {
                // The preview closed: nothing to say.
            } else if video {
                match convert(path, commands, updates, &mut wanted) {
                    Some(Ok(copy)) => {
                        updates.send_blocking(Update::Preparing(None)).ok();
                        let copy = copy.to_string_lossy().into_owned();
                        if let Err(error) = play(&copy, true, commands, updates, &mut wanted)
                            && error.code() != CLOSED
                        {
                            updates
                                .send_blocking(Update::Failed(error.message().to_string()))
                                .ok();
                        }
                    }
                    Some(Err(message)) => {
                        updates.send_blocking(Update::Failed(message)).ok();
                    }
                    // Closed while converting.
                    None => {}
                }
            } else {
                updates
                    .send_blocking(Update::Failed(error.message().to_string()))
                    .ok();
            }
        }
        unsafe { MFShutdown() }.ok();
    }

    // What `play` returns when the preview closed: not a failure, and not
    // worth converting over.
    const CLOSED: windows::core::HRESULT = windows::core::HRESULT(0x8000_4004_u32 as i32);

    /// What the preview asked of the player, carried from a failed open to
    /// the copy that plays instead.
    #[derive(Default)]
    struct Wanted {
        play: bool,
        volume: Option<f64>,
        seek: Option<f64>,
    }

    impl Wanted {
        fn take(&mut self, command: Command) {
            match command {
                Command::Play => self.play = true,
                Command::Pause => self.play = false,
                Command::Volume(volume) => self.volume = Some(volume),
                Command::Seek(seconds) => self.seek = Some(seconds),
            }
        }
    }

    // Converts `path` with FFmpeg, saying how far it got. `None` when the
    // preview closed meanwhile; commands that come are kept for the copy.
    fn convert(
        path: &str,
        commands: &mpsc::Receiver<Command>,
        updates: &async_channel::Sender<Update>,
        wanted: &mut Wanted,
    ) -> Option<Result<std::path::PathBuf, String>> {
        let closed = std::cell::Cell::new(false);
        let wanted = std::cell::RefCell::new(wanted);
        let cancelled = || {
            loop {
                match commands.try_recv() {
                    Ok(command) => wanted.borrow_mut().take(command),
                    Err(mpsc::TryRecvError::Empty) => break,
                    Err(mpsc::TryRecvError::Disconnected) => {
                        closed.set(true);
                        break;
                    }
                }
            }
            closed.get() || updates.is_closed()
        };
        let result = crate::explorer::video_proxy::make(path, &cancelled, &mut |progress| {
            updates
                .send_blocking(Update::Preparing(Some(progress)))
                .ok();
        });
        if closed.get() || updates.is_closed() {
            return None;
        }
        Some(result)
    }

    // The copy plays as made: a sound-only original stays sound-only there.
    fn path_is_proxy(path: &str) -> bool {
        std::path::Path::new(path).starts_with(nohrs_core::config::paths::cache_dir())
    }

    // Plays `path` until the preview closes. `video`: the file is a video,
    // so one that opens with a length and no picture failed (Windows read
    // its sound and not its images), and the caller converts it.
    fn play(
        path: &str,
        video: bool,
        commands: &mpsc::Receiver<Command>,
        updates: &async_channel::Sender<Update>,
        wanted: &mut Wanted,
    ) -> windows::core::Result<()> {
        let engine = unsafe {
            let factory: IMFMediaEngineClassFactory =
                CoCreateInstance(&CLSID_MFMediaEngineClassFactory, None, CLSCTX_INPROC_SERVER)?;
            let mut attributes: Option<IMFAttributes> = None;
            MFCreateAttributes(&mut attributes, 2)?;
            let attributes = attributes.ok_or_else(windows::core::Error::empty)?;
            let listener: IMFMediaEngineNotify = Listener.into();
            attributes.SetUnknown(&MF_MEDIA_ENGINE_CALLBACK, &listener)?;
            attributes.SetUINT32(&MF_MEDIA_ENGINE_VIDEO_OUTPUT_FORMAT, B8G8R8A8)?;
            let engine: IMFMediaEngine = factory.CreateInstance(0, &attributes)?;
            crate::explorer::audio_session::watch();
            engine.SetSource(&BSTR::from(path))?;
            // What was asked before this engine existed (during a failed
            // first open, or the conversion after it).
            if let Some(volume) = wanted.volume {
                engine.SetVolume(volume).ok();
            }
            if let Some(seconds) = wanted.seek {
                engine.SetCurrentTime(seconds).ok();
            }
            if wanted.play {
                engine.Play().ok();
            }
            engine
        };
        let wic: IWICImagingFactory =
            unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)? };
        let mut target: Option<(IWICBitmap, u32, u32)> = None;
        let mut last_frame = i64::MIN;
        let mut last_state = Instant::now() - Duration::from_secs(1);
        let result = loop {
            loop {
                match commands.try_recv() {
                    Ok(Command::Play) => unsafe {
                        wanted.play = true;
                        // From the end, play starts over, as players do.
                        if engine.IsEnded().as_bool() {
                            engine.SetCurrentTime(0.0).ok();
                        }
                        engine.Play().ok();
                    },
                    Ok(Command::Pause) => {
                        wanted.play = false;
                        unsafe { engine.Pause() }.unwrap_or(())
                    }
                    Ok(Command::Seek(seconds)) => {
                        wanted.seek = Some(seconds);
                        unsafe { engine.SetCurrentTime(seconds) }.unwrap_or(())
                    }
                    Ok(Command::Volume(volume)) => {
                        wanted.volume = Some(volume);
                        unsafe { engine.SetVolume(volume) }.unwrap_or(())
                    }
                    Err(mpsc::TryRecvError::Empty) => break,
                    // The preview was closed.
                    Err(mpsc::TryRecvError::Disconnected) => {
                        unsafe { engine.Shutdown() }.ok();
                        return Err(windows::core::Error::new(CLOSED, ""));
                    }
                }
            }
            if let Ok(error) = unsafe { engine.GetError() }
                && unsafe { error.GetErrorCode() } != 0
            {
                break Err(windows::core::Error::new(
                    // 0.61 hands the extended code back as a `Result`, the
                    // failure HRESULT riding in its `Err`.
                    windows::core::HRESULT::from(unsafe { error.GetExtendedErrorCode() }),
                    nohrs_core::t!("Windows não consegue reproduzir este arquivo"),
                ));
            }
            let has_video = unsafe { engine.HasVideo() }.as_bool();
            // Opened, with a length, and no picture: a video whose images
            // Windows cannot decode, played as sound only.
            if video && !has_video && unsafe { engine.GetDuration() } > 0.0 && !path_is_proxy(path)
            {
                break Err(windows::core::Error::new(
                    windows::core::HRESULT(0xC00D_5212_u32 as i32),
                    nohrs_core::t!("Windows não consegue reproduzir este arquivo"),
                ));
            }
            if has_video
                && let Ok(time) = unsafe { engine.OnVideoStreamTick() }
                && time != last_frame
                && let Some(frame) = grab(&engine, &wic, &mut target)
            {
                last_frame = time;
                if updates.send_blocking(Update::Frame(frame)).is_err() {
                    break Ok(());
                }
            }
            if last_state.elapsed() >= Duration::from_millis(100) {
                last_state = Instant::now();
                let state = unsafe {
                    Update::State {
                        position: engine.GetCurrentTime(),
                        duration: engine.GetDuration().max(0.0),
                        playing: !engine.IsPaused().as_bool() && !engine.IsEnded().as_bool(),
                        has_video,
                    }
                };
                if updates.send_blocking(state).is_err() {
                    break Ok(());
                }
            }
            // About one vsync: frames are not missed and the thread idles.
            std::thread::sleep(Duration::from_millis(8));
        };
        unsafe { engine.Shutdown() }.ok();
        result
    }

    /// Reads `path`'s sound from start to end and sends its peaks, a stretch
    /// at a time, until the file ends or the preview is closed.
    pub fn waveform(path: &str, updates: &async_channel::Sender<Update>) {
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }
            .ok()
            .ok();
        if unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) }.is_err() {
            return;
        }
        // No waveform is not worth an error over the player: it just stays empty.
        if let Err(error) = read_peaks(path, updates) {
            tracing::debug!("no waveform for {path}: {error}");
        }
        unsafe { MFShutdown() }.ok();
    }

    fn read_peaks(
        path: &str,
        updates: &async_channel::Sender<Update>,
    ) -> windows::core::Result<()> {
        let stream = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
        let reader = unsafe {
            let reader = MFCreateSourceReaderFromURL(&HSTRING::from(path), None)?;
            reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)?;
            reader.SetStreamSelection(stream, true)?;
            // Decoded to float PCM, whatever the file holds: one sample
            // format to take peaks of.
            let wanted = MFCreateMediaType()?;
            wanted.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
            wanted.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_Float)?;
            reader.SetCurrentMediaType(stream, None, &wanted)?;
            reader
        };
        let format = unsafe { reader.GetCurrentMediaType(stream)? };
        let channels = unsafe { format.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS)? }.max(1) as usize;
        let rate = unsafe { format.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND)? }.max(1) as usize;
        // Every channel's samples of a block count toward its one peak.
        let per_block = (rate / PEAK_BLOCKS_PER_SECOND).max(1) * channels;
        let (mut stretch, mut peak, mut filled) = (Vec::new(), 0.0f32, 0usize);
        let mut last_sent = Instant::now();
        loop {
            let mut flags = 0u32;
            let mut sample: Option<IMFSample> = None;
            unsafe {
                reader.ReadSample(stream, 0, None, Some(&mut flags), None, Some(&mut sample))?
            };
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                break;
            }
            let Some(sample) = sample else { continue };
            let buffer = unsafe { sample.ConvertToContiguousBuffer()? };
            let (mut data, mut length) = (std::ptr::null_mut::<u8>(), 0u32);
            unsafe { buffer.Lock(&mut data, None, Some(&mut length))? };
            if !data.is_null() {
                // Float PCM, as asked; Media Foundation buffers are aligned
                // for it. Valid until the `Unlock` below.
                let samples =
                    unsafe { std::slice::from_raw_parts(data as *const f32, length as usize / 4) };
                for &value in samples {
                    peak = peak.max(value.abs());
                    filled += 1;
                    if filled == per_block {
                        stretch.push(peak);
                        (peak, filled) = (0.0, 0);
                    }
                }
            }
            unsafe { buffer.Unlock()? };
            // A few stretches a second: the waveform visibly fills in
            // without a message per decoded buffer.
            if last_sent.elapsed() >= Duration::from_millis(200) && !stretch.is_empty() {
                last_sent = Instant::now();
                if updates
                    .send_blocking(Update::Peaks(std::mem::take(&mut stretch)))
                    .is_err()
                {
                    return Ok(());
                }
            }
        }
        if filled > 0 {
            stretch.push(peak);
        }
        updates.send_blocking(Update::Peaks(stretch)).ok();
        Ok(())
    }

    // Copies the engine's current frame into a bitmap no larger than
    // `MAX_FRAME`, keeping the aspect ratio, and returns it as an image.
    fn grab(
        engine: &IMFMediaEngine,
        wic: &IWICImagingFactory,
        target: &mut Option<(IWICBitmap, u32, u32)>,
    ) -> Option<gpui::RenderImage> {
        let (mut width, mut height) = (0u32, 0u32);
        unsafe { engine.GetNativeVideoSize(Some(&mut width), Some(&mut height)) }.ok()?;
        if width == 0 || height == 0 {
            return None;
        }
        let scale = (MAX_FRAME.0 as f64 / width as f64)
            .min(MAX_FRAME.1 as f64 / height as f64)
            .min(1.0);
        let (width, height) = (
            ((width as f64 * scale) as u32).max(1),
            ((height as f64 * scale) as u32).max(1),
        );
        if !matches!(target, Some((_, w, h)) if *w == width && *h == height) {
            let bitmap = unsafe {
                wic.CreateBitmap(
                    width,
                    height,
                    &GUID_WICPixelFormat32bppPBGRA,
                    WICBitmapCacheOnLoad,
                )
            }
            .ok()?;
            *target = Some((bitmap, width, height));
        }
        let (bitmap, _, _) = target.as_ref()?;
        let rect = RECT {
            left: 0,
            top: 0,
            right: width as i32,
            bottom: height as i32,
        };
        let black = MFARGB {
            rgbBlue: 0,
            rgbGreen: 0,
            rgbRed: 0,
            rgbAlpha: 255,
        };
        unsafe { engine.TransferVideoFrame(bitmap, None, &rect, Some(&black)) }.ok()?;
        let lock = unsafe {
            bitmap.Lock(
                &WICRect {
                    X: 0,
                    Y: 0,
                    Width: width as i32,
                    Height: height as i32,
                },
                WICBitmapLockRead.0 as u32,
            )
        }
        .ok()?;
        let stride = unsafe { lock.GetStride() }.ok()? as usize;
        let (mut size, mut data) = (0u32, std::ptr::null_mut::<u8>());
        unsafe { lock.GetDataPointer(&mut size, &mut data) }.ok()?;
        if data.is_null() {
            return None;
        }
        // The lock's buffer, valid until `lock` drops at the end of this function.
        let buffer = unsafe { std::slice::from_raw_parts(data, size as usize) };
        let row = width as usize * 4;
        let mut bgra = Vec::with_capacity(row * height as usize);
        for line in 0..height as usize {
            bgra.extend_from_slice(buffer.get(line * stride..line * stride + row)?);
        }
        crate::explorer::thumbnails::render_image(width, height, bgra)
    }
}

#[cfg(not(windows))]
mod platform {
    //! The sound is played by `ffplay`, the player of ffmpeg, and the
    //! picture of a video is drawn by `ffmpeg` itself: the frame at the
    //! playhead, scaled to the size of the preview.
    //!
    //! The engine of Windows hands the frames over as they are decoded; here
    //! the frame is asked for when the playhead moves, which is a little
    //! slower but plays every format ffmpeg reads.

    use std::process::{Child, Command as Process, Stdio};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    use super::{Command, MAX_FRAME, Update};

    /// How often the position is published and the frame asked for.
    const TICK: Duration = Duration::from_millis(40);
    /// How often a new frame is drawn while a video plays.
    const FRAME_EVERY: Duration = Duration::from_millis(200);

    pub fn run(
        path: &str,
        commands: &mpsc::Receiver<Command>,
        updates: &async_channel::Sender<Update>,
    ) {
        let Some(info) = probe(path) else {
            updates
                .send_blocking(Update::Failed("the file could not be read".into()))
                .ok();
            return;
        };
        if updates
            .send_blocking(Update::State {
                position: 0.0,
                duration: info.duration,
                playing: false,
                has_video: info.has_video,
            })
            .is_err()
        {
            return;
        }
        // The first frame, so the preview shows the video before it plays.
        if info.has_video
            && let Some(frame) = frame_at(path, 0.0)
            && updates.send_blocking(Update::Frame(frame)).is_err()
        {
            return;
        }

        let mut player: Option<Player> = None;
        let mut volume = 1.0f64;
        let mut playing = false;
        let mut position = 0.0f64;
        let mut last = Instant::now();
        let mut last_frame = Instant::now();

        loop {
            let received = commands.recv_timeout(TICK);
            match received {
                Ok(command) => match command {
                    Command::Play => {
                        playing = true;
                        last = Instant::now();
                        restart(&mut player, path, position, volume);
                    }
                    Command::Pause => {
                        playing = false;
                        stop(&mut player);
                    }
                    Command::Seek(seconds) => {
                        position = seconds.max(0.0);
                        if playing {
                            restart(&mut player, path, position, volume);
                        }
                    }
                    Command::Volume(value) => {
                        volume = value.clamp(0.0, 1.0);
                        if playing {
                            restart(&mut player, path, position, volume);
                        }
                    }
                },
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }

            if playing {
                position += last.elapsed().as_secs_f64();
                last = Instant::now();
                if info.duration > 0.0 && position >= info.duration {
                    position = info.duration;
                    playing = false;
                    stop(&mut player);
                }
                if info.has_video && last_frame.elapsed() >= FRAME_EVERY {
                    last_frame = Instant::now();
                    if let Some(frame) = frame_at(path, position) {
                        updates.send_blocking(Update::Frame(frame)).ok();
                    }
                }
            }
            if updates
                .send_blocking(Update::State {
                    position,
                    duration: info.duration,
                    playing,
                    has_video: info.has_video,
                })
                .is_err()
            {
                break;
            }
        }
        stop(&mut player);
    }

    /// The sound playing, and where it is.
    struct Player {
        child: Child,
        started: Instant,
    }

    /// Starts the sound at `position`, with the volume.
    fn restart(player: &mut Option<Player>, path: &str, position: f64, volume: f64) {
        stop(player);
        let Ok(child) = spawn_player(path, position, volume) else {
            return;
        };
        *player = Some(Player {
            child,
            started: Instant::now(),
        });
    }

    /// Starts the sound at `position`.
    fn spawn_player(path: &str, position: f64, volume: f64) -> Result<Child, std::io::Error> {
        let percent = (volume.clamp(0.0, 1.0) * 100.0).round() as u32;
        let mut command = Process::new("ffplay");
        command
            .arg("-nodisp")
            .arg("-autoexit")
            .arg("-loglevel")
            .arg("quiet")
            .arg("-volume")
            .arg(percent.to_string());
        if position > 0.0 {
            command.arg("-ss").arg(format!("{position:.3}"));
        }
        command
            .arg(path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
    }

    /// Stops the sound and frees the file it holds.
    fn stop(player: &mut Option<Player>) {
        let Some(mut current) = player.take() else {
            return;
        };
        let _ = current.child.kill();
        let _ = current.child.wait();
    }

    /// What `ffprobe` says about the file: how long it is and whether it has
    /// a picture.
    struct Info {
        duration: f64,
        has_video: bool,
    }

    fn probe(path: &str) -> Option<Info> {
        let output = Process::new("ffprobe")
            .arg("-v")
            .arg("quiet")
            .arg("-show_entries")
            .arg("format=duration:stream=codec_type")
            .arg("-of")
            .arg("default=noprint_wrappers=1")
            .arg(path)
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let text = String::from_utf8_lossy(&output.stdout);
        let mut duration = 0.0;
        let mut has_video = false;
        for line in text.lines() {
            match line.split_once('=') {
                Some(("duration", value)) => duration = value.trim().parse().unwrap_or(0.0),
                Some(("codec_type", value)) if value.trim() == "video" => has_video = true,
                _ => {}
            }
        }
        Some(Info {
            duration,
            has_video,
        })
    }

    /// The frame of the video at `position`, scaled to the size of the
    /// preview.
    fn frame_at(path: &str, position: f64) -> Option<gpui::RenderImage> {
        let mut command = Process::new("ffmpeg");
        command
            .arg("-v")
            .arg("quiet")
            .arg("-ss")
            .arg(format!("{position:.3}"))
            .arg("-i")
            .arg(path)
            .arg("-frames:v")
            .arg("1")
            .arg("-vf")
            .arg(format!(
                "scale={}:{}:force_original_aspect_ratio=decrease",
                MAX_FRAME.0, MAX_FRAME.1
            ))
            .arg("-f")
            .arg("image2pipe")
            .arg("-vcodec")
            .arg("png")
            .arg("pipe:1")
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let output = command.output().ok()?;
        if !output.status.success() || output.stdout.is_empty() {
            return None;
        }
        to_frame(&output.stdout)
    }

    /// The picture of a frame, in the layout the preview draws it in: BGRA,
    /// premultiplied.
    fn to_frame(bytes: &[u8]) -> Option<gpui::RenderImage> {
        let decoded = image::load_from_memory(bytes).ok()?.into_rgba8();
        let (width, height) = decoded.dimensions();
        let mut bgra = decoded.into_raw();
        for pixel in bgra.chunks_exact_mut(4) {
            let alpha = u16::from(pixel[3]);
            let premultiply = |channel: u8| ((u16::from(channel) * alpha + 127) / 255) as u8;
            let (red, green, blue) = (pixel[0], pixel[1], pixel[2]);
            pixel[0] = premultiply(blue);
            pixel[1] = premultiply(green);
            pixel[2] = premultiply(red);
        }
        crate::explorer::thumbnails::render_image(width, height, bgra)
    }

    /// The loudest sample of each hundredth of a second of the sound, read
    /// from the file with ffmpeg.
    fn read_peaks(path: &str) -> Result<Option<Vec<f32>>, std::io::Error> {
        use std::io::Read as _;

        const RATE: usize = 8000;
        const BLOCKS_PER_SECOND: usize = super::PEAK_BLOCKS_PER_SECOND;

        let mut child = Process::new("ffmpeg")
            .arg("-v")
            .arg("quiet")
            .arg("-i")
            .arg(path)
            .arg("-ac")
            .arg("1")
            .arg("-ar")
            .arg(RATE.to_string())
            .arg("-f")
            .arg("s16le")
            .arg("pipe:1")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let Some(mut stdout) = child.stdout.take() else {
            return Err(std::io::Error::other("the samples could not be read"));
        };
        let mut samples = Vec::new();
        let mut buffer = [0u8; 8192];
        loop {
            let read = stdout.read(&mut buffer)?;
            if read == 0 {
                break;
            }
            samples.extend_from_slice(&buffer[..read]);
        }
        let _ = child.wait();
        if samples.is_empty() {
            return Ok(None);
        }

        let block = (RATE / BLOCKS_PER_SECOND).max(1) * 2;
        let mut peaks = Vec::new();
        for chunk in samples.chunks(block) {
            let mut loudest = 0f32;
            for pair in chunk.chunks_exact(2) {
                let sample = i16::from_le_bytes([pair[0], pair[1]]);
                loudest = loudest.max(f32::from(sample.abs()) / 32768.0);
            }
            peaks.push(loudest);
        }
        Ok(Some(peaks))
    }

    /// Reads `path`'s sound from start to end and sends its peaks, a stretch
    /// at a time, until the file ends or the preview is closed.
    pub fn waveform(path: &str, updates: &async_channel::Sender<Update>) {
        let Ok(Some(peaks)) = read_peaks(path) else {
            return;
        };
        // A stretch at a time, as the engine sends them.
        for chunk in peaks.chunks(512) {
            if updates
                .send_blocking(Update::Peaks(chunk.to_vec()))
                .is_err()
            {
                return;
            }
        }
    }
}

/// The volume a sound plays at outside a pane, 0 when muted: the settings
/// page's samples sound as loud as the files do.
pub(crate) fn shared_volume() -> f64 {
    load_saved_volume();
    if MEDIA_MUTED.load(Ordering::Relaxed) {
        0.0
    } else {
        f64::from_bits(MEDIA_VOLUME.load(Ordering::Relaxed))
    }
}

#[cfg(test)]
mod tests {
    use super::{
        PEAK_BLOCKS_PER_SECOND, clock, effective_volume, is_audio, is_media, waveform_bars,
    };

    #[test]
    fn bars_take_the_loudest_peak_of_their_stretch_scaled_to_the_file() {
        // Two seconds known, one read: the second half has nothing to draw.
        let mut peaks = vec![0.1; PEAK_BLOCKS_PER_SECOND];
        peaks[10] = 0.5;
        let bars = waveform_bars(&peaks, 0.5, 2.0, 4);
        assert_eq!(bars, [Some(1.0), Some(0.2), None, None]);
        // With no length yet, what has been read fills every bar.
        assert!(
            waveform_bars(&peaks, 0.5, 0.0, 4)
                .iter()
                .all(Option::is_some)
        );
        assert!(waveform_bars(&[], 0.0, 0.0, 4).is_empty());
    }

    #[test]
    fn muting_silences_without_forgetting_the_volume() {
        assert_eq!(effective_volume(0.7, false), 0.7);
        assert_eq!(effective_volume(0.7, true), 0.0);
        assert!(is_audio("a.MP3") && !is_audio("a.mp4") && is_media("a.mp4"));
    }

    #[test]
    fn clock_reads_like_a_player() {
        assert_eq!(clock(5.4), "0:05");
        assert_eq!(clock(65.0), "1:05");
        assert_eq!(clock(3725.0), "1:02:05");
    }

    #[test]
    fn media_is_recognized_by_extension() {
        assert!(is_media("C:\\v\\clip.MP4"));
        assert!(is_media("song.flac"));
        assert!(!is_media("notes.txt"));
    }
}

// The preview's volume and mute survive a restart (issue #18), in a file of
// their own beside the mixer's (`audio_session.rs`): `0.300 1`.
fn volume_file() -> std::path::PathBuf {
    nohrs_core::config::paths::data_dir().join("preview-volume")
}

// Reads the kept volume the first time it is asked for. A small file read
// once per run; the slider moves the in-memory value from then on.
#[allow(clippy::disallowed_methods)]
fn load_saved_volume() {
    static LOADED: std::sync::Once = std::sync::Once::new();
    LOADED.call_once(|| {
        let Ok(text) = std::fs::read_to_string(volume_file()) else {
            return;
        };
        let mut parts = text.split_whitespace();
        if let Some(volume) = parts.next().and_then(|volume| volume.parse::<f64>().ok())
            && (0.0..=1.0).contains(&volume)
        {
            MEDIA_VOLUME.store(volume.to_bits(), Ordering::Relaxed);
        }
        MEDIA_MUTED.store(parts.next() == Some("1"), Ordering::Relaxed);
    });
}

fn save_volume(volume: f64, muted: bool, cx: &mut Context<ExplorerPane>) {
    let text = format!("{volume:.3} {}", u8::from(muted));
    cx.background_executor()
        .spawn(async move {
            let path = volume_file();
            let written = path
                .parent()
                .map_or(Ok(()), std::fs::create_dir_all)
                .and_then(|()| {
                    #[allow(clippy::disallowed_methods)]
                    std::fs::write(&path, text)
                });
            if let Err(error) = written {
                tracing::warn!("could not keep the preview volume: {error}");
            }
        })
        .detach();
}
