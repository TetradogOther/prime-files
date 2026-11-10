//! Milestones of the app's start, logged as milliseconds since the process
//! was created (target `nohrs::startup`), so a start can be read stage by
//! stage in the log (docs/PERFORMANCE.md, passada 10).
//!
//! Counted from the process's creation, not from `main`: what the loader
//! does before `main` is part of what the person waits for. Marks made
//! before the log exists are held and written once it does.

use std::collections::HashSet;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

static LOGGING: AtomicBool = AtomicBool::new(false);
static HELD: Mutex<Vec<(&'static str, Option<f64>)>> = Mutex::new(Vec::new());
static SEEN: Mutex<Option<HashSet<&'static str>>> = Mutex::new(None);

/// Records that the start reached `stage`.
pub fn mark(stage: &'static str) {
    let elapsed = since_process_start();
    if LOGGING.load(Ordering::Acquire) {
        write(stage, elapsed);
        return;
    }
    match HELD.lock() {
        Ok(mut held) => held.push((stage, elapsed)),
        Err(error) => tracing::warn!("startup marks poisoned: {error}"),
    }
}

/// [`mark`], only the first time `stage` is reached in this process: for a
/// stage that is a place every later frame or listing also passes.
pub fn mark_once(stage: &'static str) {
    let first = match SEEN.lock() {
        Ok(mut seen) => seen.get_or_insert_with(HashSet::new).insert(stage),
        Err(error) => {
            tracing::warn!("startup marks poisoned: {error}");
            false
        }
    };
    if first {
        mark(stage);
    }
}

/// Marks the first render of the process and, when gpui comes to draw the
/// frame after it, `first-frame`: the first one is on screen by then.
pub fn first_render(window: &gpui::Window) {
    static ASKED: AtomicBool = AtomicBool::new(false);
    if ASKED.swap(true, Ordering::AcqRel) {
        return;
    }
    mark("first-render");
    window.on_next_frame(|_, _| mark("first-frame"));
}

/// Called once the log is installed: writes the marks made before it.
pub fn logging_ready() {
    let held = match HELD.lock() {
        Ok(mut held) => std::mem::take(&mut *held),
        Err(error) => {
            tracing::warn!("startup marks poisoned: {error}");
            Vec::new()
        }
    };
    LOGGING.store(true, Ordering::Release);
    for (stage, elapsed) in held {
        write(stage, elapsed);
    }
}

fn write(stage: &str, elapsed: Option<f64>) {
    match elapsed {
        Some(ms) => {
            tracing::info!(target: "nohrs::startup", stage, ms = format!("{ms:.1}"), "startup")
        }
        None => tracing::info!(target: "nohrs::startup", stage, "startup (no process time)"),
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
fn since_process_start() -> Option<f64> {
    use windows::Win32::Foundation::FILETIME;
    use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

    let (mut created, mut exited, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    // SAFETY: the pseudo-handle of this process and four out parameters.
    unsafe {
        GetProcessTimes(
            GetCurrentProcess(),
            &mut created,
            &mut exited,
            &mut kernel,
            &mut user,
        )
    }
    .ok()?;
    let created = (u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime);
    // FILETIME counts 100 ns from 1601; `SystemTime` on Windows reads the same
    // precise clock, from 1970.
    const FROM_1601_TO_1970: u64 = 116_444_736_000_000_000;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?;
    let now = u64::try_from(now.as_nanos() / 100)
        .ok()?
        .checked_add(FROM_1601_TO_1970)?;
    Some(now.checked_sub(created)? as f64 / 10_000.0)
}

#[cfg(not(windows))]
fn since_process_start() -> Option<f64> {
    // The kernel writes the moment the process began in `/proc/self/stat`,
    // in clock ticks since the machine booted; `/proc/uptime` says how long
    // it has been up.
    let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
    // The command may hold spaces and parentheses: the fields after the
    // closing parenthesis are the ones the kernel writes.
    let fields = stat.rsplit_once(')')?.1;
    let starttime = fields.split_whitespace().nth(19)?;
    let ticks: f64 = starttime.parse().ok()?;
    let uptime = std::fs::read_to_string("/proc/uptime").ok()?;
    let seconds: f64 = uptime.split_whitespace().next()?.parse().ok()?;
    // The clock of the kernel counts in a hundredth of a second.
    let elapsed = seconds - ticks / 100.0;
    (elapsed > 0.0).then_some(elapsed * 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_process_started_before_now_and_not_long_ago() {
        let elapsed = since_process_start().expect("process time");
        assert!(elapsed > 0.0, "{elapsed}");
        // A test binary is minutes old at most, never a day.
        assert!(elapsed < 86_400_000.0, "{elapsed}");
    }
}
