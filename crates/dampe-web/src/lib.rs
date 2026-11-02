//! The manual calculator for a web page, with no video anywhere. `web/` loads this as WebAssembly: one
//! copy in a worker follows the presses, and more copies in workers of their own do the parts of a scan.
//!
//! Numbers go straight through function calls, and everything else as JSON or plain numbers in this
//! module's memory, at `out_ptr` for `out_len` bytes until the next call.

#![allow(clippy::missing_safety_doc)]

use std::cell::RefCell;
use std::fmt::Write;

use dampe_core::manual::{Event, ManualSearch, ManualSession, Press, ScanRequest, readings_from_scan, scan_part};
use dampe_core::reading::{Readings, Status};
use dampe_core::rng::STATES;

/// One read being followed.
struct Read {
    session: ManualSession,
    search: ManualSearch,
    /// The scan asked for and not back yet. Nothing else gets judged until it is.
    scanning: Option<ScanRequest>,
}

#[derive(Default)]
struct State {
    read: Option<Read>,
    out: Vec<u8>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::default();
}

fn with_state<T>(work: impl FnOnce(&mut State) -> T) -> T {
    STATE.with(|state| work(&mut state.borrow_mut()))
}

#[unsafe(no_mangle)]
pub extern "C" fn out_ptr() -> *const u8 {
    with_state(|state| state.out.as_ptr())
}

#[unsafe(no_mangle)]
pub extern "C" fn out_len() -> usize {
    with_state(|state| state.out.len())
}

/// Room for the page to write `bytes` into, for the call it's for to take back.
#[unsafe(no_mangle)]
pub extern "C" fn alloc(bytes: usize) -> *mut u8 {
    // In units of 8, so numbers of any size can be read from it
    let room = vec![0u64; bytes.div_ceil(8)];
    Box::into_raw(room.into_boxed_slice()) as *mut u8
}

/// Takes back what `alloc` gave, as `T`s.
unsafe fn take<T: Copy>(ptr: *mut u8, count: usize) -> Vec<T> {
    let bytes = count * size_of::<T>();
    let room = unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(ptr as *mut u64, bytes.div_ceil(8))) };
    let values = unsafe { std::slice::from_raw_parts(room.as_ptr() as *const T, count) };
    values.to_vec()
}

/// How many states there are to split up for scanning.
#[unsafe(no_mangle)]
pub extern "C" fn states() -> u32 {
    STATES
}

/// Starts a new read, in place of any there was.
#[unsafe(no_mangle)]
pub extern "C" fn start(press_lead_seconds: f64) {
    let (session, search) = ManualSession::without_thread(press_lead_seconds);
    with_state(|state| state.read = Some(Read { session, search, scanning: None }));
}

/// A press. 1 if it flipped the last one.
#[unsafe(no_mangle)]
pub extern "C" fn press(count: u32, seconds: f64) -> u32 {
    with_state(|state| {
        let Some(read) = &mut state.read else { return 0 };
        u32::from(read.session.press(count as u8, seconds))
    })
}

/// Goes on as of `seconds`, and writes what's new as JSON.
#[unsafe(no_mangle)]
pub extern "C" fn tick(seconds: f64) {
    with_state(|state| {
        state.out.clear();
        let Some(read) = &mut state.read else { return };
        let asked = if read.scanning.is_none() { read.search.step() } else { None };
        let mut events = Vec::new();
        read.session.tick(seconds, &mut events);
        let readings = read.session.take_readings();
        state.out = report(&events, readings.as_ref(), asked.as_ref()).into_bytes();
        if asked.is_some() {
            read.scanning = asked;
        }
    });
}

/// The results of all parts of a scan: `count` pairs of seed and Link phase as numbers, `seconds` after
/// it was asked for.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn finish_scan(ptr: *mut u8, count: usize, seconds: f64) {
    let pairs = unsafe { take::<[u32; 2]>(ptr, count) };
    with_state(|state| {
        let Some(read) = &mut state.read else { return };
        let Some(request) = read.scanning.take() else { return };
        let found: Vec<(u32, i64)> = pairs.iter().map(|&[seed, link]| (seed, i64::from(link))).collect();
        read.search.finish_scan(readings_from_scan(&found, &request.presses, request.now), seconds);
    });
}

/// Scans seeds `from..to` from `count` presses, each a frame and a count as two numbers, and writes pairs
/// of seed and Link phase as numbers.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn scan_range(from: u32, to: u32, ptr: *mut u8, count: usize, now: f64) {
    let numbers = unsafe { take::<[f64; 2]>(ptr, count) };
    let presses: Vec<Press> = numbers.iter().map(|&[frame, count]| Press { frame, count: count as u8 }).collect();
    let found = scan_part(from, to, &presses, now);
    let bytes = found.iter().flat_map(|&(seed, link)| [seed, link as u32]).flat_map(u32::to_le_bytes);
    with_state(|state| state.out = bytes.collect());
}

fn report(events: &[Event], readings: Option<&Readings>, scan: Option<&ScanRequest>) -> String {
    let mut out = String::from("{\"events\":[");
    for (i, event) in events.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        write_event(&mut out, event);
    }
    out.push_str("],\"readings\":");
    match readings {
        Some(readings) => write_readings(&mut out, readings),
        None => out.push_str("null"),
    }
    out.push_str(",\"scan\":");
    match scan {
        Some(scan) => write_scan(&mut out, scan),
        None => out.push_str("null"),
    }
    out.push('}');
    out
}

/// How far off the moment to press can be, either way, in milliseconds.
fn margin_ms(window: &dampe_core::reading::Window) -> f64 {
    (window.last_roll - window.first_roll + 1) as f64 / dampe_core::manual::GAME_FPS / 2.0 * 1000.0
}

fn write_event(out: &mut String, event: &Event) {
    let _ = match event {
        Event::Status(Status::Collecting(n)) => write!(out, r#"{{"t":"collecting","n":{n}}}"#),
        Event::Status(Status::Narrowing(n)) => write!(out, r#"{{"t":"narrowing","n":{n}}}"#),
        Event::Status(Status::Confirming(_) | Status::Locked) => write!(out, r#"{{"t":"locked"}}"#),
        Event::Cue { seconds, window } => {
            write!(out, r#"{{"t":"cue","seconds":{seconds},"margin":{}}}"#, margin_ms(window))
        }
        Event::LikelyCue { seconds, window, chance } => {
            write!(out, r#"{{"t":"likely","seconds":{seconds},"margin":{},"chance":{chance}}}"#, margin_ms(window))
        }
        Event::LikelyChance { chance } => write!(out, r#"{{"t":"chance","chance":{chance}}}"#),
        Event::LikelyCancelled => write!(out, r#"{{"t":"likelyCancelled"}}"#),
        Event::LikelyEnded => write!(out, r#"{{"t":"likelyEnded"}}"#),
        Event::CueCancelled => write!(out, r#"{{"t":"cueCancelled"}}"#),
    };
}

fn write_readings(out: &mut String, readings: &Readings) {
    let _ = write!(out, r#"{{"total":{},"notFound":{},"top":["#, readings.total, readings.not_found);
    for (i, reading) in readings.top.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let window = reading.next_window.map(|window| ManualSession::window_seconds(&window));
        let _ = write!(
            out,
            r#"{{"share":{},"seed":{},"link":{},"hits":{},"misses":{},"strays":{},"proven":{},"seconds":{}}}"#,
            reading.share,
            reading.candidate.seed,
            reading.candidate.link_first,
            reading.hits,
            reading.misses,
            reading.extras,
            reading.confirmed > 0,
            window.map_or("null".to_owned(), |seconds| seconds.to_string()),
        );
    }
    out.push_str("]}");
}

fn write_scan(out: &mut String, scan: &ScanRequest) {
    let _ = write!(out, r#"{{"now":{},"presses":["#, scan.now);
    for (i, press) in scan.presses.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let _ = write!(out, "{},{}", press.frame, press.count);
    }
    out.push_str("]}");
}
