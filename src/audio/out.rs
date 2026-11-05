//! A sound output that keeps playing when the device goes away: the cores' mixers render into a
//! cpal stream on the default output device, and Windows moves the default (or drops the device)
//! when a headset's audio connects or disconnects, a USB or Bluetooth device comes and goes, or
//! the user picks another output. A stream on the old device then plays into nothing, for good.
//! [`Output`] owns the stream on a thread of its own, checks once a second, and opens a new one on
//! the default device when the old one reported an error or the default is another device: the
//! way the game's own sound engine follows the default.
//!
//! The core supplies how a stream is built on a device (its mixer callback); every rebuild calls
//! it again, so each stream renders the core's own shared mixer from where it is.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

/// How often the device is checked.
const CHECK: Duration = Duration::from_secs(1);

/// A stream on `device`: the stream (not yet playing), its sample rate and channels. Set `lost`
/// from the stream's error callback, so the next check opens it again.
pub trait Build: FnMut(&cpal::Device, Arc<AtomicBool>) -> Result<(cpal::Stream, u32, u16), String> + Send + 'static {}
impl<F> Build for F where F: FnMut(&cpal::Device, Arc<AtomicBool>) -> Result<(cpal::Stream, u32, u16), String> + Send + 'static {}

/// The output stream, kept on the default device. Dropping it stops the sound.
pub struct Output {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    /// The first stream's rate and channels (a later device may differ; each stream renders at its own).
    pub rate: u32,
    pub channels: u16,
}

struct Open {
    _stream: cpal::Stream,
    device: Option<String>,
    lost: Arc<AtomicBool>,
}

fn open(build: &mut impl Build) -> Result<(Open, u32, u16), String> {
    let device = cpal::default_host().default_output_device().ok_or("no output device")?;
    let name = device.name().ok();
    let lost = Arc::new(AtomicBool::new(false));
    let (stream, rate, channels) = build(&device, lost.clone())?;
    stream.play().map_err(|e| format!("play: {e}"))?;
    Ok((Open { _stream: stream, device: name, lost }, rate, channels))
}

fn default_name() -> Option<String> {
    cpal::default_host().default_output_device()?.name().ok()
}

impl Output {
    /// Opens the first stream on the default device (`Err` says why there's none) and keeps it
    /// there from a thread named `name`.
    pub fn start(name: &str, mut build: impl Build) -> Result<Output, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel::<Result<(u32, u16), String>>();
        let st = stop.clone();
        let label = name.to_owned();
        let thread = std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                let mut current = match open(&mut build) {
                    Ok((o, rate, channels)) => {
                        let _ = tx.send(Ok((rate, channels)));
                        Some(o)
                    }
                    Err(e) => {
                        let _ = tx.send(Err(e));
                        return;
                    }
                };
                let mut failed = false;
                while !st.load(Ordering::Relaxed) {
                    std::thread::park_timeout(CHECK);
                    if st.load(Ordering::Relaxed) {
                        break;
                    }
                    let now = default_name();
                    let reopen = match &current {
                        None => now.is_some(),
                        Some(o) => o.lost.load(Ordering::Relaxed) || (now.is_some() && now != o.device),
                    };
                    if !reopen {
                        continue;
                    }
                    // The old stream goes first: some drivers hold the device until it's closed.
                    drop(current.take());
                    match open(&mut build) {
                        Ok((o, rate, channels)) => {
                            eprintln!("{label}: sound moved to {} ({rate} Hz, {channels} ch)", o.device.as_deref().unwrap_or("the default device"));
                            current = Some(o);
                            failed = false;
                        }
                        Err(e) => {
                            // Tried again each check until a device is there.
                            if !failed {
                                eprintln!("{label}: no sound output for now: {e}");
                            }
                            failed = true;
                        }
                    }
                }
                drop(current);
            })
            .map_err(|e| format!("audio thread: {e}"))?;
        match rx.recv() {
            Ok(Ok((rate, channels))) => Ok(Output { stop, thread: Some(thread), rate, channels }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => Err("audio thread died".into()),
        }
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            t.thread().unpark();
            let _ = t.join();
        }
    }
}
