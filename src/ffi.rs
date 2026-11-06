//! Panic guard for the cores' C ABI exports.
//!
//! A Rust panic that reaches an `extern "C"` boundary aborts the whole process, so one stray
//! `unwrap()` in a sim would take the host game down with it. Every export runs its body
//! through [`guard`]: a panic is caught there, its message and source location are kept for
//! the host to read ([`take_panic`]), and the export returns the fallback it names (0, null,
//! -1: whatever that export already uses for "failed" or "none").
//!
//! A handle whose call panicked may be left half-updated. Later calls on it are still guarded,
//! so the worst case is more logged panics and fallback values, never a crash.

use std::cell::RefCell;
use std::collections::VecDeque;
use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, Once};

/// Messages kept for the host; older ones are dropped first.
const KEEP: usize = 16;

static PANICS: Mutex<VecDeque<String>> = Mutex::new(VecDeque::new());
static COUNT: AtomicU64 = AtomicU64::new(0);
static HOOK: Once = Once::new();

thread_local! {
    /// The last panic on this thread as the hook saw it ("panicked at file:line:col: msg").
    static LAST: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// The hook adds the location the payload lacks; the previous hook still runs.
fn install_hook() {
    HOOK.call_once(|| {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            let _ = LAST.try_with(|l| *l.borrow_mut() = Some(info.to_string()));
            previous(info);
        }));
    });
}

fn payload_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "panic with a non-string payload".to_owned())
}

fn record(message: String) {
    COUNT.fetch_add(1, Ordering::Relaxed);
    let mut q = PANICS.lock().unwrap_or_else(|e| e.into_inner());
    if q.len() == KEEP {
        q.pop_front();
    }
    q.push_back(message);
}

/// Runs an export's body. On a panic, records "`name`: panicked at file:line: message" and
/// returns `fallback` instead of unwinding into the host.
pub fn guard<R>(name: &'static str, fallback: R, body: impl FnOnce() -> R) -> R {
    install_hook();
    match panic::catch_unwind(AssertUnwindSafe(body)) {
        Ok(r) => r,
        Err(payload) => {
            let detail = LAST
                .try_with(|l| l.borrow_mut().take())
                .ok()
                .flatten()
                .unwrap_or_else(|| payload_text(payload.as_ref()));
            record(format!("{name}: {detail}"));
            fallback
        }
    }
}

/// The oldest panic message not yet taken, if any.
pub fn take_panic() -> Option<String> {
    PANICS.lock().unwrap_or_else(|e| e.into_inner()).pop_front()
}

/// Panics caught since the library loaded (including messages already taken or dropped).
pub fn panic_count() -> u64 {
    COUNT.load(Ordering::Relaxed)
}

/// Takes the oldest panic message into `out` as UTF-8 (cut at a character boundary to fit,
/// then a NUL when there is room). Returns the bytes written, 0 when there is none.
pub fn take_panic_into(out: &mut [u8]) -> u32 {
    let Some(msg) = take_panic() else { return 0 };
    let room = out.len().saturating_sub(1);
    let mut n = msg.len().min(room);
    while !msg.is_char_boundary(n) {
        n -= 1;
    }
    out[..n].copy_from_slice(&msg.as_bytes()[..n]);
    if n < out.len() {
        out[n] = 0;
    }
    n as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    // The queue is process-wide; one test exercises it end to end so parallel tests don't race.
    #[test]
    fn a_panic_becomes_the_fallback_and_a_located_message() {
        while take_panic().is_some() {}
        let before = panic_count();
        assert_eq!(guard("ok_export", 0u32, || 7), 7);
        let v: Vec<u32> = Vec::new();
        #[allow(clippy::unnecessary_operation)]
        let got = guard("bad_export", u32::MAX, || v[3]);
        assert_eq!(got, u32::MAX);
        assert_eq!(guard("unit_export", (), || panic!("boom {}", 1)), ());
        assert_eq!(panic_count(), before + 2);

        let first = take_panic().unwrap();
        assert!(first.starts_with("bad_export: "), "{first}");
        assert!(first.contains("ffi.rs"), "no location: {first}");
        assert!(first.contains("index out of bounds"), "{first}");

        let mut small = [0xAAu8; 16];
        let n = take_panic_into(&mut small) as usize;
        assert_eq!(n, 15);
        assert_eq!(small[15], 0);
        assert_eq!(&small[..12], b"unit_export:");
        assert_eq!(take_panic_into(&mut small), 0);
    }
}
