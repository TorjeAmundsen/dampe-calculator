//! DirectShow capture, adapted from AutoSplit-rs's `autosplit-capture`. The graph setup is the same.
//! What differs is the end of it: AutoSplit only wants the newest frame, and a read needs all of
//! them in order, so each frame's brightness goes into a queue with the time it arrived.
//!
//! All COM work happens on threads this module owns.

#![allow(non_snake_case)]

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;

use windows::Win32::Foundation::{E_POINTER, S_OK};
use windows::Win32::Media::DirectShow::{
    EC_DEVICE_LOST, IAMStreamConfig, IBaseFilter, ICaptureGraphBuilder2, ICreateDevEnum, IGraphBuilder, IMediaControl,
    IMediaEventEx, IMediaFilter, IMediaSample, PINDIR_OUTPUT,
};
use windows::Win32::Media::MediaFoundation::{AM_MEDIA_TYPE, VIDEOINFOHEADER};
use windows::Win32::System::Com::StructuredStorage::IPropertyBag;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize,
    IEnumMoniker, IMoniker,
};
use windows::Win32::System::Variant::{VARIANT, VariantClear};
use windows::core::{BOOL, GUID, HRESULT, IUnknown, IUnknown_Vtbl, Interface, PCWSTR, implement, interface, w};

const fn guid(value: u128) -> GUID {
    GUID::from_u128(value)
}

/// The GUID DirectShow uses for a FOURCC media subtype.
const fn fourcc(code: &[u8; 4]) -> GUID {
    let data1 = u32::from_le_bytes(*code) as u128;
    GUID::from_u128((data1 << 96) | 0x0000_0010_8000_00aa_0038_9b71)
}

const CLSID_SYSTEM_DEVICE_ENUM: GUID = guid(0x62be5d10_60eb_11d0_bd3b_00a0c911ce86);
const CLSID_VIDEO_INPUT_DEVICE_CATEGORY: GUID = guid(0x860bb310_5d01_11d0_bd3b_00a0c911ce86);
const CLSID_CAPTURE_GRAPH_BUILDER2: GUID = guid(0xbf87b6e1_8c27_11d0_b3f0_00aa003761c5);
const CLSID_FILTER_GRAPH: GUID = guid(0xe436ebb3_524f_11ce_9f53_0020af0ba770);
// qedit.h, no longer in the Windows SDK but qedit.dll still ships with Windows
const CLSID_SAMPLE_GRABBER: GUID = guid(0xc1f400a0_3f08_11d3_9f0b_006008039e37);
const CLSID_NULL_RENDERER: GUID = guid(0xc1f400a4_3f08_11d3_9f0b_006008039e37);
const PIN_CATEGORY_CAPTURE: GUID = guid(0xfb6c4281_0353_11d1_905f_0000c0cc16ba);
const PIN_CATEGORY_PREVIEW: GUID = guid(0xfb6c4282_0353_11d1_905f_0000c0cc16ba);
const MEDIATYPE_VIDEO: GUID = fourcc(b"vids");
const FORMAT_VIDEO_INFO: GUID = guid(0x05589f80_c356_11ce_bf01_00aa0055595a);
const MEDIASUBTYPE_RGB24: GUID = guid(0xe436eb7d_524f_11ce_9f53_0020af0ba770);
const MEDIASUBTYPE_NV12: GUID = fourcc(b"NV12");

/// Devices that accept any format but only really send NV12.
const NV12_DEVICES: [&str; 2] = ["OBS Virtual Camera", "Streamlabs Desktop Virtual Webcam"];

/// Frames queued for the reader. It keeps up unless something's wrong, and this is about a second.
const QUEUE_FRAMES: usize = 60;

/// OBS Virtual Camera's frames are filled with this while it isn't started.
const PLACEHOLDER_BYTE: u8 = 127;

#[interface("0579154A-2B53-4994-B0D0-E773148EFF85")]
unsafe trait ISampleGrabberCB: IUnknown {
    fn SampleCB(&self, sample_time: f64, sample: *mut c_void) -> HRESULT;
    fn BufferCB(&self, sample_time: f64, buffer: *mut u8, length: i32) -> HRESULT;
}

#[interface("6B652FFF-11FE-4fce-92AD-0266B5D7C78F")]
unsafe trait ISampleGrabber: IUnknown {
    fn SetOneShot(&self, one_shot: BOOL) -> HRESULT;
    fn SetMediaType(&self, media_type: *const AM_MEDIA_TYPE) -> HRESULT;
    fn GetConnectedMediaType(&self, media_type: *mut AM_MEDIA_TYPE) -> HRESULT;
    fn SetBufferSamples(&self, buffer_samples: BOOL) -> HRESULT;
    fn GetCurrentBuffer(&self, size: *mut i32, buffer: *mut i32) -> HRESULT;
    fn GetCurrentSample(&self, sample: *mut *mut c_void) -> HRESULT;
    fn SetCallback(&self, callback: *mut c_void, which_method: i32) -> HRESULT;
}

/// The current thread's membership of the multithreaded COM apartment, left when dropped.
struct ComApartment {
    initialized: bool,
}

impl ComApartment {
    fn enter() -> Self {
        Self { initialized: unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok() }
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        if self.initialized {
            unsafe { CoUninitialize() };
        }
    }
}

fn monikers() -> windows::core::Result<Vec<(IMoniker, String)>> {
    unsafe {
        let device_enum: ICreateDevEnum = CoCreateInstance(&CLSID_SYSTEM_DEVICE_ENUM, None, CLSCTX_INPROC_SERVER)?;
        let mut enum_moniker: Option<IEnumMoniker> = None;
        // S_FALSE means there are no devices, and leaves the enumerator empty
        device_enum.CreateClassEnumerator(&CLSID_VIDEO_INPUT_DEVICE_CATEGORY, &mut enum_moniker, 0)?;
        let Some(enum_moniker) = enum_moniker else {
            return Ok(Vec::new());
        };
        let mut devices = Vec::new();
        loop {
            let mut moniker = [None];
            if enum_moniker.Next(&mut moniker, None) != S_OK {
                break;
            }
            let Some(moniker) = moniker[0].take() else { break };
            let Ok(bag) = moniker.BindToStorage::<_, _, IPropertyBag>(None, None) else {
                continue;
            };
            let mut name = VARIANT::default();
            let read = bag.Read(w!("FriendlyName"), &mut name, None);
            let friendly_name = if read.is_ok() { name.to_string() } else { String::new() };
            let _ = VariantClear(&mut name);
            devices.push((moniker, friendly_name));
        }
        Ok(devices)
    }
}

/// The names of the video devices, in the system's order.
pub fn list_devices() -> Result<Vec<String>, String> {
    std::thread::spawn(|| {
        let _apartment = ComApartment::enter();
        monikers().map(|m| m.into_iter().map(|(_, name)| name).collect()).map_err(|error| error.message())
    })
    .join()
    .unwrap_or_else(|_| Err("listing devices crashed".into()))
}

/// A frame's brightness, one byte per pixel, top row first.
pub struct CapturedFrame {
    pub luma: Vec<u8>,
    /// U and V, alternating, for every 2 by 2 pixels. Empty unless color was asked for.
    pub chroma: Vec<u8>,
    pub width: usize,
    pub height: usize,
    pub arrival: Instant,
}

#[derive(Clone, Copy, PartialEq)]
enum Format {
    Nv12,
    /// B, G, R, bottom row first.
    Rgb24,
}

/// What the capture thread reports to whoever reads the frames.
#[derive(Default)]
pub struct CaptureState {
    pub dropped: AtomicU64,
    pub placeholder: AtomicBool,
    pub failed: AtomicBool,
}

#[implement(ISampleGrabberCB)]
struct GrabberCallback {
    frames: SyncSender<CapturedFrame>,
    state: Arc<CaptureState>,
    width: usize,
    height: usize,
    format: Format,
    color: bool,
    spare: Mutex<Vec<Vec<u8>>>,
}

impl GrabberCallback {
    fn receive(&self, data: &[u8]) {
        let (width, height) = (self.width, self.height);
        let arrival = Instant::now();
        let mut luma = self.spare.lock().unwrap().pop().unwrap_or_default();
        luma.resize(width * height, 0);
        match self.format {
            Format::Nv12 => {
                let y = &data[..width * height];
                if y[0] == PLACEHOLDER_BYTE && y[width - 1] == PLACEHOLDER_BYTE && y[y.len() - 1] == PLACEHOLDER_BYTE {
                    self.state.placeholder.store(true, Ordering::Relaxed);
                    return;
                }
                luma.copy_from_slice(y);
            }
            Format::Rgb24 => {
                let stride = (width * 3).div_ceil(4) * 4;
                for (out_row, row) in luma.chunks_exact_mut(width).zip(data.chunks_exact(stride).rev()) {
                    for (out, pixel) in out_row.iter_mut().zip(row.as_chunks::<3>().0) {
                        // BT.709, video range, like the NV12 path
                        let weighted = 47 * u32::from(pixel[2]) + 157 * u32::from(pixel[1]) + 16 * u32::from(pixel[0]);
                        *out = (16 + (weighted >> 8)) as u8;
                    }
                }
            }
        }
        self.state.placeholder.store(false, Ordering::Relaxed);
        let frame = CapturedFrame { luma, chroma: self.chroma(data), width, height, arrival };
        match self.frames.try_send(frame) {
            Ok(()) => {}
            Err(TrySendError::Full(frame) | TrySendError::Disconnected(frame)) => {
                self.state.dropped.fetch_add(1, Ordering::Relaxed);
                self.spare.lock().unwrap().push(frame.luma);
            }
        }
    }

    fn chroma(&self, data: &[u8]) -> Vec<u8> {
        if !self.color {
            return Vec::new();
        }
        let (width, height) = (self.width, self.height);
        match self.format {
            Format::Nv12 => data[width * height..].to_vec(),
            Format::Rgb24 => {
                let stride = (width * 3).div_ceil(4) * 4;
                let rows = data.chunks_exact(stride).rev().step_by(2);
                let mut chroma = Vec::with_capacity(width.div_ceil(2) * height.div_ceil(2) * 2);
                for row in rows {
                    for pixel in row.as_chunks::<3>().0.iter().take(width).step_by(2) {
                        let (b, g, r) = (i32::from(pixel[0]), i32::from(pixel[1]), i32::from(pixel[2]));
                        chroma.push(((-26 * r - 87 * g + 112 * b) / 256 + 128) as u8);
                        chroma.push(((112 * r - 102 * g - 10 * b) / 256 + 128) as u8);
                    }
                }
                chroma
            }
        }
    }

    fn expected_len(&self) -> usize {
        match self.format {
            Format::Nv12 => self.width * self.height * 3 / 2,
            Format::Rgb24 => (self.width * 3).div_ceil(4) * 4 * self.height,
        }
    }
}

impl ISampleGrabberCB_Impl for GrabberCallback_Impl {
    unsafe fn SampleCB(&self, _sample_time: f64, sample: *mut c_void) -> HRESULT {
        let Some(sample) = (unsafe { IMediaSample::from_raw_borrowed(&sample) }) else {
            return E_POINTER;
        };
        let Ok(pointer) = (unsafe { sample.GetPointer() }) else { return S_OK };
        let length = unsafe { sample.GetActualDataLength() }.max(0) as usize;
        if pointer.is_null() || length != self.expected_len() {
            return S_OK;
        }
        self.receive(unsafe { std::slice::from_raw_parts(pointer, length) });
        S_OK
    }

    unsafe fn BufferCB(&self, _sample_time: f64, _buffer: *mut u8, _length: i32) -> HRESULT {
        windows::Win32::Foundation::E_NOTIMPL
    }
}

/// Owns an `AM_MEDIA_TYPE` allocated by DirectShow (`DeleteMediaType`).
struct MediaType(*mut AM_MEDIA_TYPE);

impl MediaType {
    fn info(&mut self) -> &mut VIDEOINFOHEADER {
        unsafe { &mut *((*self.0).pbFormat as *mut VIDEOINFOHEADER) }
    }
}

impl Drop for MediaType {
    fn drop(&mut self) {
        unsafe {
            if self.0.is_null() {
                return;
            }
            let media_type = &mut *self.0;
            if media_type.cbFormat != 0 {
                CoTaskMemFree(Some(media_type.pbFormat as *const c_void));
            }
            if let Some(unknown) = media_type.pUnk.take() {
                drop(unknown);
            }
            CoTaskMemFree(Some(self.0 as *const c_void));
        }
    }
}

fn set_nv12(config: &IAMStreamConfig, media_type: &mut MediaType, width: i32, height: i32) -> bool {
    unsafe {
        let info = media_type.info();
        info.bmiHeader.biWidth = width;
        info.bmiHeader.biHeight = height;
        info.rcSource.right = width;
        info.rcTarget.right = width;
        info.rcSource.bottom = height;
        info.rcTarget.bottom = height;
        let raw = &mut *media_type.0;
        raw.formattype = FORMAT_VIDEO_INFO;
        raw.majortype = MEDIATYPE_VIDEO;
        raw.subtype = MEDIASUBTYPE_NV12;
        raw.lSampleSize = 0;
        config.SetFormat(media_type.0).is_ok()
    }
}

struct Graph {
    control: IMediaControl,
    events: IMediaEventEx,
}

/// The current size of the device's first output pin: what it was last set to, by OBS for example.
fn first_output_size(source: &IBaseFilter) -> Option<(i32, i32)> {
    unsafe {
        let pins = source.EnumPins().ok()?;
        loop {
            let mut pin = [None];
            if pins.Next(&mut pin, None) != S_OK {
                return None;
            }
            let pin = pin[0].take()?;
            if pin.QueryDirection().ok()? != PINDIR_OUTPUT {
                continue;
            }
            let config: IAMStreamConfig = pin.cast().ok()?;
            let mut format = MediaType(config.GetFormat().ok()?);
            let header = format.info().bmiHeader;
            return Some((header.biWidth, header.biHeight));
        }
    }
}

fn build_graph(
    moniker: &IMoniker,
    name: &str,
    frames: SyncSender<CapturedFrame>,
    state: Arc<CaptureState>,
    color: bool,
) -> windows::core::Result<(Graph, (usize, usize))> {
    unsafe {
        let builder: ICaptureGraphBuilder2 =
            CoCreateInstance(&CLSID_CAPTURE_GRAPH_BUILDER2, None, CLSCTX_INPROC_SERVER)?;
        let graph: IGraphBuilder = CoCreateInstance(&CLSID_FILTER_GRAPH, None, CLSCTX_INPROC_SERVER)?;
        let events: IMediaEventEx = graph.cast()?;
        builder.SetFiltergraph(&graph)?;
        let control: IMediaControl = graph.cast()?;

        let source: IBaseFilter = moniker.BindToObject(None, None)?;
        let wide_name: Vec<u16> = name.encode_utf16().chain([0]).collect();
        graph.AddFilter(&source, PCWSTR(wide_name.as_ptr()))?;

        let find_config = |category: &GUID| -> windows::core::Result<IAMStreamConfig> {
            let mut config: *mut c_void = std::ptr::null_mut();
            builder.FindInterface(
                Some(category),
                Some(&MEDIATYPE_VIDEO),
                &source,
                &IAMStreamConfig::IID,
                &mut config,
            )?;
            Ok(IAMStreamConfig::from_raw(config))
        };
        let config = find_config(&PIN_CATEGORY_PREVIEW).or_else(|_| find_config(&PIN_CATEGORY_CAPTURE))?;

        let mut media_type = MediaType(config.GetFormat()?);
        let current = media_type.info().bmiHeader;
        let (width, height) = first_output_size(&source).unwrap_or((current.biWidth, current.biHeight));
        let nv12 = NV12_DEVICES.contains(&name) && set_nv12(&config, &mut media_type, width, height);
        drop(media_type);

        let grabber_filter: IBaseFilter = CoCreateInstance(&CLSID_SAMPLE_GRABBER, None, CLSCTX_INPROC_SERVER)?;
        graph.AddFilter(&grabber_filter, w!("Sample Grabber"))?;
        let grabber: ISampleGrabber = grabber_filter.cast()?;
        grabber.SetOneShot(false.into()).ok()?;
        grabber.SetBufferSamples(false.into()).ok()?;

        let size = (width.max(0) as usize, height.unsigned_abs() as usize);
        let callback: ISampleGrabberCB = GrabberCallback {
            frames,
            state,
            width: size.0,
            height: size.1,
            format: if nv12 { Format::Nv12 } else { Format::Rgb24 },
            color,
            spare: Mutex::default(),
        }
        .into();
        grabber.SetCallback(callback.as_raw(), 0).ok()?;

        // NV12 stays NV12, everything else becomes RGB24 through Windows' own conversion filters
        let grabber_type = AM_MEDIA_TYPE {
            majortype: MEDIATYPE_VIDEO,
            subtype: if nv12 { MEDIASUBTYPE_NV12 } else { MEDIASUBTYPE_RGB24 },
            formattype: FORMAT_VIDEO_INFO,
            ..Default::default()
        };
        grabber.SetMediaType(&grabber_type).ok()?;

        let renderer: IBaseFilter = CoCreateInstance(&CLSID_NULL_RENDERER, None, CLSCTX_INPROC_SERVER)?;
        graph.AddFilter(&renderer, w!("NullRenderer"))?;
        builder.RenderStream(Some(&PIN_CATEGORY_PREVIEW), &MEDIATYPE_VIDEO, &source, &grabber_filter, &renderer)?;

        // Deliver frames as soon as they arrive instead of waiting on a reference clock
        if let Ok(filter) = graph.cast::<IMediaFilter>() {
            let _ = filter.SetSyncSource(None);
        }
        control.Run()?;
        Ok((Graph { control, events }, size))
    }
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
                let _apartment = ComApartment::enter();
                let built = monikers().map_err(|e| e.message()).and_then(|monikers| {
                    let (moniker, _) = monikers
                        .into_iter()
                        .find(|(_, candidate)| *candidate == name)
                        .ok_or_else(|| "the device is not connected".to_owned())?;
                    build_graph(&moniker, &name, sender, thread_state.clone(), color).map_err(|e| e.message())
                });
                let graph = match built {
                    Ok((graph, size)) => {
                        let _ = ready_sender.send(Ok(size));
                        graph
                    }
                    Err(error) => {
                        let _ = ready_sender.send(Err(error));
                        return;
                    }
                };
                while !thread_stop.load(Ordering::Relaxed) {
                    let (mut code, mut param1, mut param2) = (0, 0, 0);
                    // Waits for an event, or times out to check `stop`
                    if unsafe { graph.events.GetEvent(&mut code, &mut param1, &mut param2, 250) }.is_ok() {
                        unsafe {
                            let _ = graph.events.FreeEventParams(code, param1, param2);
                        }
                        if code == EC_DEVICE_LOST as i32 && param2 == 0 {
                            thread_state.failed.store(true, Ordering::Relaxed);
                        }
                    }
                }
                unsafe {
                    let _ = graph.control.Stop();
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
