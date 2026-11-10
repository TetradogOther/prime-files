//! The quick search's shortcut, heard anywhere: Ctrl pressed twice, as
//! Listary does, Shift or Alt twice, or a combination such as Ctrl+Space.
//!
//! A double press is not a shortcut Windows can register (`RegisterHotKey`
//! takes a key with modifiers), so the keyboard is watched with a low-level
//! hook, on a thread of its own, as Listary and PowerToys do. The hook only
//! looks: a double press goes on to the program in front, as with Listary. A
//! combination is taken, so that Ctrl+Space does not also type a space.
//!
//! The setting is `[launcher] hotkey`: `off`, `double-ctrl`, `double-shift`,
//! `double-alt`, a combination (`ctrl+space`, `ctrl+shift+f`, `alt+f1`), or
//! `auto`, the default: Ctrl twice, or Shift twice when Listary is installed,
//! so that the two do not open together.

use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use gpui::App;

/// The longest gap between the two presses of a double press, as Listary's.
const TAP_WINDOW: Duration = Duration::from_millis(400);

/// What opens the quick search.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Trigger {
    /// No shortcut.
    Off,
    /// A modifier pressed twice on its own.
    Double(Tap),
    /// A key with modifiers.
    Chord(Chord),
}

/// A modifier pressed twice on its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tap {
    /// Either Ctrl.
    Ctrl,
    /// Either Shift.
    Shift,
    /// Either Alt.
    Alt,
}

/// A key with the modifiers held for it; `vk` is the Windows key code.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chord {
    /// Ctrl held.
    pub ctrl: bool,
    /// Shift held.
    pub shift: bool,
    /// Alt held.
    pub alt: bool,
    /// A Windows key held.
    pub win: bool,
    /// The key's Windows code.
    pub vk: u16,
}

// Key names in the setting, and their Windows codes.
const KEYS: &[(&str, u16)] = &[
    ("space", 0x20),
    ("enter", 0x0D),
    ("tab", 0x09),
    ("backspace", 0x08),
    ("delete", 0x2E),
    ("insert", 0x2D),
    ("home", 0x24),
    ("end", 0x23),
    ("pageup", 0x21),
    ("pagedown", 0x22),
    ("left", 0x25),
    ("up", 0x26),
    ("right", 0x27),
    ("down", 0x28),
];

fn vk_of(name: &str) -> Option<u16> {
    if let Some(&(_, vk)) = KEYS.iter().find(|(key, _)| *key == name) {
        return Some(vk);
    }
    let mut chars = name.chars();
    if let (Some(c), None) = (chars.next(), chars.clone().next()) {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            return Some(c.to_ascii_uppercase() as u16);
        }
        return None;
    }
    let number: u16 = name.strip_prefix('f')?.parse().ok()?;
    (1..=24).contains(&number).then(|| 0x6F + number)
}

fn name_of(vk: u16) -> String {
    if let Some((key, _)) = KEYS.iter().find(|(_, code)| *code == vk) {
        return key.to_string();
    }
    match vk {
        0x30..=0x39 | 0x41..=0x5A => ((vk as u8) as char).to_ascii_lowercase().to_string(),
        0x70..=0x87 => format!("f{}", vk - 0x6F),
        _ => format!("{vk:#04x}"),
    }
}

/// The setting as the hook takes it: lower case, `cmd` read as Ctrl, and
/// `auto` decided.
pub fn resolve(text: &str) -> String {
    let text = text
        .trim()
        .to_lowercase()
        .replace("cmd", "ctrl")
        .replace(' ', "");
    // `Cmd+Shift+Space` was the default written to every config file before
    // the quick search, when nothing let anyone choose it: it counts as the
    // default it was, not as a choice.
    if text.is_empty() || text == "auto" || text == "ctrl+shift+space" {
        // Settings resolve it on every frame they draw: Listary's file is
        // looked for once.
        static LISTARY: OnceLock<bool> = OnceLock::new();
        let listary = *LISTARY.get_or_init(|| crate::listary::preferences_file().is_some());
        return if listary {
            "double-shift"
        } else {
            "double-ctrl"
        }
        .to_string();
    }
    text
}

/// Reads a (resolved) setting; `None` when it names no shortcut.
pub fn parse(text: &str) -> Option<Trigger> {
    match text {
        "off" | "none" => return Some(Trigger::Off),
        "double-ctrl" => return Some(Trigger::Double(Tap::Ctrl)),
        "double-shift" => return Some(Trigger::Double(Tap::Shift)),
        "double-alt" => return Some(Trigger::Double(Tap::Alt)),
        _ => {}
    }
    let mut chord = Chord {
        ctrl: false,
        shift: false,
        alt: false,
        win: false,
        vk: 0,
    };
    let parts: Vec<&str> = text.split('+').filter(|part| !part.is_empty()).collect();
    let (key, modifiers) = parts.split_last()?;
    for modifier in modifiers {
        match *modifier {
            "ctrl" | "control" => chord.ctrl = true,
            "shift" => chord.shift = true,
            "alt" => chord.alt = true,
            "win" | "super" | "meta" => chord.win = true,
            _ => return None,
        }
    }
    chord.vk = vk_of(key)?;
    // A key alone would be taken from every program; a function key is the
    // one key that may go without modifiers.
    let bare = !(chord.ctrl || chord.shift || chord.alt || chord.win);
    if bare && !(0x70..=0x87).contains(&chord.vk) {
        return None;
    }
    Some(Trigger::Chord(chord))
}

/// The setting written back from a trigger.
pub fn to_setting(trigger: Trigger) -> String {
    match trigger {
        Trigger::Off => "off".into(),
        Trigger::Double(Tap::Ctrl) => "double-ctrl".into(),
        Trigger::Double(Tap::Shift) => "double-shift".into(),
        Trigger::Double(Tap::Alt) => "double-alt".into(),
        Trigger::Chord(chord) => {
            let mut parts = Vec::new();
            if chord.ctrl {
                parts.push("ctrl".to_string());
            }
            if chord.shift {
                parts.push("shift".to_string());
            }
            if chord.alt {
                parts.push("alt".to_string());
            }
            if chord.win {
                parts.push("win".to_string());
            }
            parts.push(name_of(chord.vk));
            parts.join("+")
        }
    }
}

/// The shortcut as people read it: "Ctrl 2x", "Ctrl+Espaço".
pub fn describe(text: &str) -> String {
    match parse(&resolve(text)) {
        None => text.to_string(),
        Some(Trigger::Off) => nohrs_core::t!("Desligado").to_string(),
        Some(Trigger::Double(Tap::Ctrl)) => "Ctrl 2x".into(),
        Some(Trigger::Double(Tap::Shift)) => "Shift 2x".into(),
        Some(Trigger::Double(Tap::Alt)) => "Alt 2x".into(),
        Some(Trigger::Chord(chord)) => {
            let mut parts: Vec<String> = Vec::new();
            for (on, name) in [
                (chord.ctrl, "Ctrl"),
                (chord.shift, "Shift"),
                (chord.alt, "Alt"),
                (chord.win, "Win"),
            ] {
                if on {
                    parts.push(name.into());
                }
            }
            let key = name_of(chord.vk);
            parts.push(match key.as_str() {
                "space" => nohrs_core::t!("Espaço").to_string(),
                _ => {
                    let mut chars = key.chars();
                    chars
                        .next()
                        .map(|first| first.to_uppercase().chain(chars).collect())
                        .unwrap_or_default()
                }
            });
            parts.join("+")
        }
    }
}

/// The setting for a combination pressed while one is being recorded
/// (Settings): gpui's key name and the modifiers held. `None` for a key the
/// hook cannot take, or one that would take ordinary typing.
pub fn from_keystroke(ctrl: bool, shift: bool, alt: bool, win: bool, key: &str) -> Option<String> {
    let key = key.to_lowercase();
    let vk = vk_of(&key)?;
    let chord = Chord {
        ctrl,
        shift,
        alt,
        win,
        vk,
    };
    let text = to_setting(Trigger::Chord(chord));
    // Shift alone with a letter or digit is typing a capital or a symbol.
    let typing = !(ctrl || alt || win) && (0x30..=0x5A).contains(&vk);
    (!typing && parse(&text).is_some()).then_some(text)
}

static TRIGGER: Mutex<Trigger> = Mutex::new(Trigger::Off);
static SENDER: OnceLock<async_channel::Sender<Signal>> = OnceLock::new();
// Whether the quick search is open: Esc then closes it, wherever the
// keyboard is, and goes nowhere else (in an Open dialog it would cancel it).
static ESCAPE_CLOSES: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

// What the hook tells the UI thread.
enum Signal {
    Shortcut,
    Escape,
}

/// Tells the hook whether the quick search is open, for Esc.
pub fn set_open(open: bool) {
    ESCAPE_CLOSES.store(open, std::sync::atomic::Ordering::Relaxed);
}

fn trigger() -> Trigger {
    TRIGGER
        .lock()
        .map(|trigger| *trigger)
        .unwrap_or(Trigger::Off)
}

/// Uses the setting `text` from now on.
pub fn set(text: &str) {
    let trigger = parse(&resolve(text)).unwrap_or_else(|| {
        tracing::warn!("launcher.hotkey {text:?} names no shortcut; the quick search has none");
        Trigger::Off
    });
    if let Ok(mut held) = TRIGGER.lock() {
        *held = trigger;
    }
}

/// Starts listening to the keyboard, once per run; `on_fire` runs on the UI
/// thread each time the shortcut is pressed, `on_escape` each time Esc is
/// pressed while the quick search is open (`set_open`).
pub fn install(
    cx: &mut App,
    on_fire: impl Fn(&mut App) + 'static,
    on_escape: impl Fn(&mut App) + 'static,
) {
    let (sender, receiver) = async_channel::unbounded::<Signal>();
    if SENDER.set(sender).is_err() {
        return;
    }
    if !platform::start() {
        return;
    }
    cx.spawn(async move |cx| {
        while let Ok(signal) = receiver.recv().await {
            let done = cx.update(|cx| match signal {
                Signal::Shortcut => on_fire(cx),
                Signal::Escape => on_escape(cx),
            });
            if done.is_err() {
                break;
            }
        }
    })
    .detach();
}

fn send(signal: Signal) {
    if let Some(sender) = SENDER.get() {
        sender.try_send(signal).ok();
    }
}

fn fire() {
    send(Signal::Shortcut);
}

// The keyboard as the hook sees it, between two events.
#[derive(Default)]
struct Keys {
    // The modifier down, and whether nothing else was pressed since.
    held: Option<Tap>,
    clean: bool,
    // The last press of a modifier on its own, and when it ended.
    last_tap: Option<(Tap, std::time::Instant)>,
    // A key whose press was taken: its release is taken too.
    swallow_up: Option<u16>,
}

fn tap_of(vk: u16) -> Option<Tap> {
    match vk {
        0x11 | 0xA2 | 0xA3 => Some(Tap::Ctrl),
        0x10 | 0xA0 | 0xA1 => Some(Tap::Shift),
        0x12 | 0xA4 | 0xA5 => Some(Tap::Alt),
        _ => None,
    }
}

impl Keys {
    // One key event; `held` says which modifiers are down for a combination.
    // Returns whether the event is taken from the program in front.
    fn key(&mut self, vk: u16, down: bool, held: impl Fn() -> (bool, bool, bool, bool)) -> bool {
        let trigger = trigger();
        if !down && self.swallow_up == Some(vk) {
            self.swallow_up = None;
            return true;
        }
        const ESCAPE: u16 = 0x1B;
        if down && vk == ESCAPE && ESCAPE_CLOSES.load(std::sync::atomic::Ordering::Relaxed) {
            self.swallow_up = Some(ESCAPE);
            send(Signal::Escape);
            return true;
        }
        let tap = tap_of(vk);
        if down {
            match tap {
                Some(tap) => {
                    // Two modifiers together are a combination being held,
                    // not a press of either on its own. Auto-repeat sends
                    // more presses of the one held; they change nothing.
                    let other = self.held.is_some_and(|before| before != tap);
                    if self.held != Some(tap) {
                        self.held = Some(tap);
                        self.clean = !other;
                    }
                    if other {
                        self.last_tap = None;
                    }
                }
                None => {
                    self.clean = false;
                    self.last_tap = None;
                    if let Trigger::Chord(chord) = trigger {
                        let (ctrl, shift, alt, win) = held();
                        if chord.vk == vk
                            && (ctrl, shift, alt, win)
                                == (chord.ctrl, chord.shift, chord.alt, chord.win)
                        {
                            self.swallow_up = Some(vk);
                            fire();
                            return true;
                        }
                    }
                }
            }
        } else if let Some(tap) = tap
            && self.held == Some(tap)
        {
            self.held = None;
            if self.clean && trigger == Trigger::Double(tap) {
                let now = std::time::Instant::now();
                match self.last_tap {
                    Some((last, at)) if last == tap && now.duration_since(at) <= TAP_WINDOW => {
                        self.last_tap = None;
                        fire();
                    }
                    _ => self.last_tap = Some((tap, now)),
                }
            }
        }
        false
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod platform {
    //! The low-level keyboard hook. `unsafe` stays here.

    use std::cell::RefCell;

    use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, VK_CONTROL, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, DispatchMessageW, GetMessageW, HC_ACTION, KBDLLHOOKSTRUCT, LLKHF_INJECTED,
        MSG, SetWindowsHookExW, TranslateMessage, WH_KEYBOARD_LL, WM_KEYDOWN, WM_SYSKEYDOWN,
    };

    use super::Keys;

    thread_local! {
        static KEYS: RefCell<Keys> = RefCell::new(Keys::default());
    }

    fn pressed(vk: u16) -> bool {
        unsafe { GetAsyncKeyState(i32::from(vk)) as u16 & 0x8000 != 0 }
    }

    fn held() -> (bool, bool, bool, bool) {
        (
            pressed(VK_CONTROL.0),
            pressed(VK_SHIFT.0),
            pressed(VK_MENU.0),
            pressed(VK_LWIN.0) || pressed(VK_RWIN.0),
        )
    }

    unsafe extern "system" fn hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code == HC_ACTION as i32 {
            let info = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
            // Keys sent by programs (ours too) are not the person typing.
            if info.flags.0 & LLKHF_INJECTED.0 == 0 {
                let message = wparam.0 as u32;
                let down = message == WM_KEYDOWN || message == WM_SYSKEYDOWN;
                let taken = KEYS.with(|keys| keys.borrow_mut().key(info.vkCode as u16, down, held));
                if taken {
                    return LRESULT(1);
                }
            }
        }
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }

    /// The hook runs on the thread that set it, from that thread's message
    /// loop, which Windows waits on for every key: the thread does nothing
    /// else, so typing never waits on Prime Files.
    pub fn start() -> bool {
        std::thread::Builder::new()
            .name("nohrs-quick-keys".into())
            .spawn(|| unsafe {
                if SetWindowsHookExW(WH_KEYBOARD_LL, Some(hook), None, 0).is_err() {
                    tracing::warn!("quick search shortcut disabled: SetWindowsHookExW failed");
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
}

#[cfg(not(windows))]
mod platform {
    pub fn start() -> bool {
        false
    }
}

#[cfg(test)]
mod tests {
    use super::{Chord, Keys, TRIGGER, Tap, Trigger, from_keystroke, parse, to_setting};

    #[test]
    fn settings_read_and_write_back() {
        assert_eq!(parse("double-ctrl"), Some(Trigger::Double(Tap::Ctrl)));
        assert_eq!(parse("off"), Some(Trigger::Off));
        let chord = Chord {
            ctrl: true,
            shift: false,
            alt: false,
            win: false,
            vk: 0x20,
        };
        assert_eq!(parse("ctrl+space"), Some(Trigger::Chord(chord)));
        assert_eq!(to_setting(Trigger::Chord(chord)), "ctrl+space");
        assert_eq!(
            parse("ctrl+shift+f"),
            Some(Trigger::Chord(Chord {
                shift: true,
                vk: 0x46,
                ..chord
            }))
        );
        assert_eq!(parse("f9").map(to_setting), Some("f9".to_string()));
        // A letter alone would be taken from every program.
        assert_eq!(parse("a"), None);
        assert_eq!(parse("hyper+space"), None);
        assert_eq!(
            from_keystroke(false, true, false, false, "space"),
            Some("shift+space".into())
        );
        assert_eq!(from_keystroke(false, true, false, false, "a"), None);
        assert_eq!(
            from_keystroke(true, false, true, false, "k"),
            Some("ctrl+alt+k".into())
        );
    }

    // Serial: they share the process-wide trigger.
    #[test]
    fn presses_on_their_own_count_and_others_do_not() {
        let none = || (false, false, false, false);
        let ctrl_down = || (true, false, false, false);
        *TRIGGER.lock().unwrap() = Trigger::Double(Tap::Ctrl);
        let mut keys = Keys::default();
        // Ctrl, Ctrl: the second release fires; nothing is taken.
        assert!(!keys.key(0xA2, true, none));
        assert!(!keys.key(0xA2, false, none));
        assert!(keys.last_tap.is_some());
        assert!(!keys.key(0xA2, true, none));
        assert!(!keys.key(0xA2, false, none));
        assert!(keys.last_tap.is_none());
        // Ctrl+C in between is a copy, not a press of Ctrl on its own.
        let mut keys = Keys::default();
        keys.key(0xA2, true, none);
        keys.key(0xA2, false, none);
        keys.key(0xA2, true, none);
        keys.key(0x43, true, ctrl_down);
        keys.key(0x43, false, ctrl_down);
        keys.key(0xA2, false, none);
        assert!(keys.last_tap.is_none());
        // A combination is taken, press and release.
        *TRIGGER.lock().unwrap() = Trigger::Chord(super::Chord {
            ctrl: true,
            shift: false,
            alt: false,
            win: false,
            vk: 0x20,
        });
        let mut keys = Keys::default();
        keys.key(0xA2, true, none);
        assert!(keys.key(0x20, true, ctrl_down));
        assert!(keys.key(0x20, false, ctrl_down));
        // Space alone is typing.
        assert!(!keys.key(0x20, true, none));
        *TRIGGER.lock().unwrap() = Trigger::Off;
    }
}
