//! V4L2 capture for Linux, adapted from AutoSplit-rs's `autosplit-capture`, with the same
//! `Capture` as the DirectShow code. Only raw formats: reading MJPEG would need libjpeg-turbo, and
//! OBS Virtual Camera and most capture cards offer something raw.

use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, TrySendError, sync_channel};
use std::thread::JoinHandle;
use std::time::Instant;

use linuxvideo::format::{PixFormat, PixelFormat as Fourcc};
use linuxvideo::stream::ReadStream;
use linuxvideo::{BufType, CapabilityFlags, Device};

/// Frames waiting to be read, about 1 second of them, like the DirectShow code.
const QUEUE_FRAMES: usize = 60;

/// A frame's brightness, one byte per pixel, top row first.
pub struct CapturedFrame {
    pub luma: Vec<u8>,
    /// U and V, alternating, for every 2 by 2 pixels. Empty unless color was asked for.
    pub chroma: Vec<u8>,
    pub width: usize,
    pub height: usize,
    pub arrival: Instant,
}

/// What the capture thread reports to whoever reads the frames.
#[derive(Default)]
pub struct CaptureState {
    pub dropped: AtomicU64,
    pub placeholder: AtomicBool,
    pub failed: AtomicBool,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Format {
    Nv12,
    Nv21,
    Yu12,
    Yv12,
    Yuyv,
    Uyvy,
}

const FORMATS: [(&[u8; 4], Format); 6] = [
    (b"NV12", Format::Nv12),
    (b"YUYV", Format::Yuyv),
    (b"UYVY", Format::Uyvy),
    (b"YU12", Format::Yu12),
    (b"YV12", Format::Yv12),
    (b"NV21", Format::Nv21),
];

fn format_of(fourcc: Fourcc) -> Option<Format> {
    FORMATS.iter().find(|(code, _)| **code == fourcc.as_fourcc()).map(|&(_, format)| format)
}

#[derive(Clone, Copy)]
struct Layout {
    format: Format,
    width: usize,
    height: usize,
    /// Bytes per row of the first plane.
    stride: usize,
}

impl Layout {
    fn frame_len(&self) -> usize {
        let chroma_height = self.height.div_ceil(2);
        match self.format {
            Format::Nv12 | Format::Nv21 => self.stride * (self.height + chroma_height),
            Format::Yu12 | Format::Yv12 => self.stride * self.height + self.stride.div_ceil(2) * chroma_height * 2,
            Format::Yuyv | Format::Uyvy => self.stride * self.height,
        }
    }

    fn luma(&self, data: &[u8], luma: &mut Vec<u8>) {
        luma.clear();
        let rows = data.chunks_exact(self.stride).take(self.height);
        match self.format {
            Format::Nv12 | Format::Nv21 | Format::Yu12 | Format::Yv12 => {
                rows.for_each(|row| luma.extend_from_slice(&row[..self.width]));
            }
            Format::Yuyv => rows.for_each(|row| luma.extend(row.iter().step_by(2).take(self.width))),
            Format::Uyvy => rows.for_each(|row| luma.extend(row.iter().skip(1).step_by(2).take(self.width))),
        }
    }

    fn chroma(&self, data: &[u8]) -> Vec<u8> {
        let (columns, rows) = (self.width.div_ceil(2), self.height.div_ceil(2));
        let mut chroma = Vec::with_capacity(columns * rows * 2);
        let first_plane = self.stride * self.height;
        match self.format {
            Format::Nv12 | Format::Nv21 => {
                for row in data[first_plane..].chunks_exact(self.stride).take(rows) {
                    for pair in row[..columns * 2].chunks_exact(2) {
                        let (u, v) = if self.format == Format::Nv12 { (pair[0], pair[1]) } else { (pair[1], pair[0]) };
                        chroma.extend_from_slice(&[u, v]);
                    }
                }
            }
            Format::Yu12 | Format::Yv12 => {
                let half = self.stride.div_ceil(2);
                let (first, second) = data[first_plane..].split_at(half * rows);
                let (u, v) = if self.format == Format::Yu12 { (first, second) } else { (second, first) };
                for (u_row, v_row) in u.chunks_exact(half).zip(v.chunks_exact(half)).take(rows) {
                    for (u, v) in u_row.iter().zip(v_row).take(columns) {
                        chroma.extend_from_slice(&[*u, *v]);
                    }
                }
            }
            Format::Yuyv | Format::Uyvy => {
                let (u_at, v_at) = if self.format == Format::Yuyv { (1, 3) } else { (0, 2) };
                for row in data.chunks_exact(self.stride).take(self.height).step_by(2) {
                    for pixels in row[..columns * 4].chunks_exact(4) {
                        chroma.extend_from_slice(&[pixels[u_at], pixels[v_at]]);
                    }
                }
            }
        }
        chroma
    }
}

fn device_path(index: usize) -> PathBuf {
    PathBuf::from(format!("/dev/video{index}"))
}

/// Metadata nodes, which UVC webcams add next to each camera, can't capture video.
///
/// A v4l2loopback node, like OBS Virtual Camera's, counts too. OBS makes it with exclusive_caps, so
/// it only says it captures while OBS writes to it.
fn captures_video(path: &Path) -> bool {
    match Device::open(path).and_then(|device| device.capabilities()) {
        Ok(capabilities) => {
            capabilities.device_capabilities().contains(CapabilityFlags::VIDEO_CAPTURE)
                || capabilities.driver() == "v4l2 loopback"
        }
        // Listed anyway, so opening it explains the problem (like missing permissions)
        Err(_) => true,
    }
}

/// The nodes that capture video and their names, in order.
fn nodes() -> Result<Vec<(PathBuf, String)>, String> {
    let entries = match std::fs::read_dir("/sys/class/video4linux") {
        Ok(entries) => entries,
        // The class only exists once a video device driver is loaded
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.to_string()),
    };
    let mut nodes: Vec<(usize, String)> = entries
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let index = entry.file_name().to_str()?.strip_prefix("video")?.parse().ok()?;
            let name = std::fs::read_to_string(entry.path().join("name")).ok()?.trim().to_owned();
            Some((index, name))
        })
        .collect();
    nodes.sort();
    let paths = nodes.into_iter().map(|(index, name)| (device_path(index), name));
    Ok(paths.filter(|(path, _)| captures_video(path)).collect())
}

/// The names of the video devices, in the system's order.
pub fn list_devices() -> Result<Vec<String>, String> {
    Ok(nodes()?.into_iter().map(|(_, name)| name).collect())
}

/// Starts `device` in the first format it can do, trying its current one first.
fn start(device: &str) -> Result<(ReadStream, Layout), String> {
    let (path, _) = nodes()?
        .into_iter()
        .find(|(_, name)| name == device)
        .ok_or_else(|| "the device is not connected".to_owned())?;
    let opened = Device::open(&path).map_err(|error| error.to_string())?;
    let current = match opened.format(BufType::VIDEO_CAPTURE).map_err(|error| error.to_string())? {
        linuxvideo::format::Format::VideoCapture(format) => format,
        _ => return Err("not a video capture device".to_owned()),
    };
    let (width, height) = (current.width(), current.height());
    let mut candidates = vec![current.pixel_format()];
    candidates.extend(FORMATS.iter().map(|(code, _)| Fourcc::from_fourcc(**code)));
    candidates.retain(|candidate| format_of(*candidate).is_some());

    let mut device = opened;
    let mut last_error = "it only offers formats this can't read, like MJPEG".to_owned();
    for candidate in candidates {
        match device.video_capture(PixFormat::new(width, height, candidate)) {
            Ok(capture) => {
                let negotiated = capture.format();
                if let Some(format) = format_of(negotiated.pixel_format()) {
                    let (width, height) = (negotiated.width() as usize, negotiated.height() as usize);
                    let packed = matches!(format, Format::Yuyv | Format::Uyvy);
                    let stride = (negotiated.bytes_per_line() as usize).max(if packed { width * 2 } else { width });
                    let layout = Layout { format, width, height, stride };
                    return capture.into_stream().map(|stream| (stream, layout)).map_err(|error| error.to_string());
                }
            }
            Err(error) => last_error = error.to_string(),
        }
        device = Device::open(&path).map_err(|error| error.to_string())?;
    }
    Err(last_error)
}

/// An open device, capturing until dropped.
pub struct Capture {
    pub frames: Receiver<CapturedFrame>,
    pub state: Arc<CaptureState>,
    pub size: (usize, usize),
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Capture {
    /// Opens `device`. Frames only have color when `color` says so, since most uses don't need it.
    pub fn open(device: &str, color: bool) -> Result<Self, String> {
        let (sender, frames) = sync_channel(QUEUE_FRAMES);
        let (ready_sender, ready) = sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let state = Arc::new(CaptureState::default());
        let (thread_stop, thread_state, name) = (stop.clone(), state.clone(), device.to_owned());
        let thread = std::thread::Builder::new()
            .name("capture".into())
            .spawn(move || {
                let (mut stream, layout) = match start(&name) {
                    Ok(started) => started,
                    Err(error) => {
                        let _ = ready_sender.send(Err(error));
                        return;
                    }
                };
                let _ = ready_sender.send(Ok((layout.width, layout.height)));
                let fd = stream.as_raw_fd();
                let mut spare = Vec::new();
                while !thread_stop.load(Ordering::Relaxed) {
                    // Waits with a timeout to check `stop`, and not hang on a device that stopped
                    let mut pollfd = libc::pollfd { fd, events: libc::POLLIN, revents: 0 };
                    let ready = unsafe { libc::poll(&mut pollfd, 1, 250) };
                    if ready < 0 || pollfd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                        thread_state.failed.store(true, Ordering::Relaxed);
                        return;
                    }
                    if ready == 0 {
                        continue;
                    }
                    let result = stream.dequeue(|buffer| {
                        let arrival = Instant::now();
                        let data: &[u8] = &buffer;
                        if buffer.is_error() || data.len() < layout.frame_len() {
                            return Ok(());
                        }
                        let mut luma: Vec<u8> = spare.pop().unwrap_or_default();
                        layout.luma(data, &mut luma);
                        let chroma = if color { layout.chroma(data) } else { Vec::new() };
                        let (width, height) = (layout.width, layout.height);
                        match sender.try_send(CapturedFrame { luma, chroma, width, height, arrival }) {
                            Ok(()) => {}
                            Err(TrySendError::Full(frame) | TrySendError::Disconnected(frame)) => {
                                thread_state.dropped.fetch_add(1, Ordering::Relaxed);
                                spare.push(frame.luma);
                            }
                        }
                        Ok(())
                    });
                    if result.is_err() {
                        thread_state.failed.store(true, Ordering::Relaxed);
                        return;
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        let size = ready.recv().map_err(|_| "the capture thread stopped".to_owned())??;
        Ok(Self { frames, state, size, stop, thread: Some(thread) })
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 4 by 2 with 2 bytes of padding per row: Y is 1 to 8, the blocks' U are 10, 20 and V 30, 40.
    fn converted(format: Format, data: &[u8], stride: usize) -> (Vec<u8>, Vec<u8>) {
        let layout = Layout { format, width: 4, height: 2, stride };
        assert_eq!(layout.frame_len(), data.len(), "{format:?}");
        let mut luma = Vec::new();
        layout.luma(data, &mut luma);
        (luma, layout.chroma(data))
    }

    #[test]
    fn every_format_gives_the_same_planes() {
        let y = [1, 2, 3, 4, 0, 0, 5, 6, 7, 8, 0, 0];
        let nv12 = [&y[..], &[10, 30, 20, 40, 0, 0]].concat();
        let nv21 = [&y[..], &[30, 10, 40, 20, 0, 0]].concat();
        let yu12 = [&y[..], &[10, 20, 0], &[30, 40, 0]].concat();
        let yv12 = [&y[..], &[30, 40, 0], &[10, 20, 0]].concat();
        #[rustfmt::skip]
        let yuyv = [1, 10, 2, 30, 3, 20, 4, 40, 0, 0, 5, 10, 6, 30, 7, 20, 8, 40, 0, 0];
        #[rustfmt::skip]
        let uyvy = [10, 1, 30, 2, 20, 3, 40, 4, 0, 0, 10, 5, 30, 6, 20, 7, 40, 8, 0, 0];
        let cases: [(Format, &[u8], usize); 6] = [
            (Format::Nv12, &nv12, 6),
            (Format::Nv21, &nv21, 6),
            (Format::Yu12, &yu12, 6),
            (Format::Yv12, &yv12, 6),
            (Format::Yuyv, &yuyv, 10),
            (Format::Uyvy, &uyvy, 10),
        ];
        for (format, data, stride) in cases {
            let (luma, chroma) = converted(format, data, stride);
            assert_eq!(luma, [1, 2, 3, 4, 5, 6, 7, 8], "{format:?}");
            assert_eq!(chroma, [10, 30, 20, 40], "{format:?}");
        }
    }
}
