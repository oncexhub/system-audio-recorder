//! System-audio capture with WASAPI loopback.
//!
//! Loopback taps the *output* of the default playback device (exactly what
//! you hear), never a microphone. A few details make long recordings solid:
//!
//! * A silent render stream keeps the audio engine running, so loopback keeps
//!   delivering (silent) packets when nothing plays. Without it Windows sends
//!   no data during silence and the recording would lose those gaps.
//! * Everything is converted to one fixed output format (48 kHz, stereo,
//!   16-bit), so switching headphones/speakers mid-recording is seamless.
//! * Device loss or a change of default device just reopens the new default
//!   device; any gap is filled with silence to keep the timeline intact.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use windows::core::GUID;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::*;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;

use crate::sink::{Format, Sink, CHANNELS, SAMPLE_RATE};

pub const WM_RECORDER_FAILED: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 10;

const WAVE_FORMAT_PCM_TAG: u16 = 1;
const WAVE_FORMAT_IEEE_FLOAT_TAG: u16 = 3;
const WAVE_FORMAT_EXTENSIBLE_TAG: u16 = 0xFFFE;
const SUBTYPE_PCM: GUID = GUID::from_u128(0x00000001_0000_0010_8000_00aa00389b71);
const SUBTYPE_FLOAT: GUID = GUID::from_u128(0x00000003_0000_0010_8000_00aa00389b71);
const AUTOCONVERTPCM: u32 = 0x8000_0000;
const SRC_DEFAULT_QUALITY: u32 = 0x0800_0000;
const BUFFER_SILENT: u32 = 0x2;
const HNS_PER_SEC: i64 = 10_000_000;

pub struct Recorder {
    stop: Arc<AtomicBool>,
    frames: Arc<AtomicU64>,
    levels: Arc<Mutex<Vec<f32>>>,
    thread: Option<JoinHandle<Result<(), String>>>,
    pub path: PathBuf,
}

impl Recorder {
    /// Starts recording. Returns once the file and audio device are open, so
    /// any setup error is reported immediately.
    pub fn start(path: PathBuf, format: Format, kbps: u32, notify: HWND) -> Result<Recorder, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let frames = Arc::new(AtomicU64::new(0));
        let levels = Arc::new(Mutex::new(Vec::new()));
        let (tx, rx) = mpsc::channel();
        let (s, f, p, l) = (stop.clone(), frames.clone(), path.clone(), levels.clone());
        let hwnd = notify.0 as isize;
        let thread = std::thread::Builder::new()
            .name("capture".into())
            .spawn(move || {
                let r = run(&p, format, kbps, &s, &f, &l, tx);
                if r.is_err() && !s.load(Ordering::SeqCst) {
                    unsafe {
                        let _ = PostMessageW(Some(HWND(hwnd as _)), WM_RECORDER_FAILED, WPARAM(0), LPARAM(0));
                    }
                }
                r
            })
            .map_err(|e| e.to_string())?;

        match rx.recv_timeout(Duration::from_secs(10)) {
            Ok(Ok(())) => Ok(Recorder { stop, frames, levels, thread: Some(thread), path }),
            Ok(Err(e)) => {
                let _ = thread.join();
                let _ = std::fs::remove_file(&path);
                Err(e)
            }
            Err(_) => {
                stop.store(true, Ordering::SeqCst);
                Err("The audio device did not respond.".into())
            }
        }
    }

    pub fn seconds(&self) -> u64 {
        self.frames.load(Ordering::Relaxed) / SAMPLE_RATE as u64
    }

    /// Audio levels (0..1, one per 40 ms) produced since the last call.
    pub fn take_levels(&self) -> Vec<f32> {
        self.levels.lock().map(|mut l| std::mem::take(&mut *l)).unwrap_or_default()
    }

    /// Stops recording and finalizes the file.
    pub fn stop(mut self) -> Result<PathBuf, String> {
        self.stop.store(true, Ordering::SeqCst);
        let r = match self.thread.take() {
            Some(t) => t.join().unwrap_or_else(|_| Err("Recorder crashed.".into())),
            None => Ok(()),
        };
        r.map(|_| self.path.clone())
    }
}

fn run(
    path: &PathBuf,
    format: Format,
    kbps: u32,
    stop: &AtomicBool,
    frames: &AtomicU64,
    levels: &Mutex<Vec<f32>>,
    ready: mpsc::Sender<Result<(), String>>,
) -> Result<(), String> {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    raise_thread_priority();

    let mut sink = match Sink::create(path, format, kbps) {
        Ok(s) => s,
        Err(e) => {
            let _ = ready.send(Err(e.clone()));
            return Err(e);
        }
    };
    let dev = match Loopback::open() {
        Ok(d) => d,
        Err(e) => {
            let e = format!("Could not open the audio output device.\n\n{e}");
            let _ = ready.send(Err(e.clone()));
            let _ = sink.finish();
            return Err(e);
        }
    };
    let _ = ready.send(Ok(()));

    let mut device: Option<Loopback> = Some(dev);
    let mut out: Vec<i16> = Vec::with_capacity(SAMPLE_RATE as usize);
    let mut written: u64 = 0;
    let mut gap_since: Option<Instant> = None;
    let mut gap_written: u64 = 0;
    let mut last_check = Instant::now();
    let mut result = Ok(());
    let mut meter = Meter::default();

    while !stop.load(Ordering::SeqCst) {
        std::thread::sleep(Duration::from_millis(10));

        if let Some(d) = device.as_mut() {
            out.clear();
            let drained = d.drain(&mut out);
            if !out.is_empty() {
                if let Err(e) = sink.write(&out) {
                    result = Err(e);
                    break;
                }
                written += (out.len() / CHANNELS as usize) as u64;
                frames.store(written, Ordering::Relaxed);
                meter.feed(&out, levels);
            }
            let mut reopen = drained.is_err();
            if !reopen && last_check.elapsed() > Duration::from_secs(1) {
                last_check = Instant::now();
                reopen = default_device_id().map_or(false, |id| id != d.id);
            }
            if reopen {
                device = None;
                gap_since = Some(Instant::now());
                gap_written = 0;
            }
        }

        if device.is_none() {
            // Keep the timeline going with silence until a device is back.
            let since = *gap_since.get_or_insert_with(Instant::now);
            let due = (since.elapsed().as_secs_f64() * SAMPLE_RATE as f64) as u64;
            if due > gap_written {
                let n = (due - gap_written) as usize;
                out.clear();
                out.resize(n * CHANNELS as usize, 0);
                if let Err(e) = sink.write(&out) {
                    result = Err(e);
                    break;
                }
                gap_written = due;
                written += n as u64;
                frames.store(written, Ordering::Relaxed);
            }
            if last_check.elapsed() > Duration::from_millis(500) {
                last_check = Instant::now();
                if let Ok(d) = Loopback::open() {
                    device = Some(d);
                    gap_since = None;
                }
            }
        }
    }

    drop(device);
    let fin = sink.finish();
    unsafe { CoUninitialize() };
    result.and(fin)
}

/// Turns samples into one display level per 40 ms (RMS, mapped from -50..-6 dBFS).
#[derive(Default)]
struct Meter {
    sum: f64,
    count: usize,
}

impl Meter {
    fn feed(&mut self, samples: &[i16], levels: &Mutex<Vec<f32>>) {
        const BLOCK: usize = (SAMPLE_RATE as usize / 25) * CHANNELS as usize;
        let mut new = Vec::new();
        for &v in samples {
            let f = v as f64 / 32768.0;
            self.sum += f * f;
            self.count += 1;
            if self.count == BLOCK {
                let rms = (self.sum / BLOCK as f64).sqrt() as f32;
                let db = 20.0 * rms.max(1e-6).log10();
                new.push(((db + 50.0) / 44.0).clamp(0.0, 1.0));
                self.sum = 0.0;
                self.count = 0;
            }
        }
        if !new.is_empty() {
            if let Ok(mut l) = levels.lock() {
                // The UI drains this regularly; cap it in case the window is hidden.
                if l.len() > 500 {
                    l.clear();
                }
                l.extend(new);
            }
        }
    }
}

fn raise_thread_priority() {
    use windows::Win32::System::Threading::*;
    unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_TIME_CRITICAL);
    }
}

fn enumerator() -> windows::core::Result<IMMDeviceEnumerator> {
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
}

fn default_device_id() -> Option<String> {
    unsafe {
        let dev = enumerator().ok()?.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
        device_id(&dev)
    }
}

unsafe fn device_id(dev: &IMMDevice) -> Option<String> {
    let p = dev.GetId().ok()?;
    let s = p.to_string().ok();
    CoTaskMemFree(Some(p.0 as _));
    s
}

struct Loopback {
    id: String,
    client: IAudioClient,
    capture: IAudioCaptureClient,
    conv: Converter,
    // Silent playback stream that keeps loopback packets flowing.
    keepalive: Option<(IAudioClient, IAudioRenderClient, u32)>,
}

impl Loopback {
    fn open() -> Result<Loopback, String> {
        unsafe { Self::open_inner() }.map_err(|e| e.message().to_string())
    }

    unsafe fn open_inner() -> windows::core::Result<Loopback> {
        let dev = enumerator()?.GetDefaultAudioEndpoint(eRender, eConsole)?;
        let id = device_id(&dev).unwrap_or_default();
        let client: IAudioClient = dev.Activate(CLSCTX_ALL, None)?;

        // Preferred: let Windows convert to 48 kHz stereo float for us.
        let want = float_stereo_48k();
        let buffer = 2 * HNS_PER_SEC;
        let conv = match client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK | AUTOCONVERTPCM | SRC_DEFAULT_QUALITY,
            buffer,
            0,
            &want.Format,
            None,
        ) {
            Ok(()) => Converter::new(SampleKind::F32, 2, SAMPLE_RATE),
            Err(_) => {
                // Fallback: use the device's own mix format and convert here.
                let mix = client.GetMixFormat()?;
                let conv = Converter::from_format(&*mix);
                let r = client.Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, buffer, 0, mix, None);
                CoTaskMemFree(Some(mix as _));
                r?;
                conv.ok_or_else(|| windows::core::Error::new(windows::core::HRESULT(0x88890008u32 as i32), "Unsupported audio format"))?
            }
        };
        let capture: IAudioCaptureClient = client.GetService()?;

        let keepalive = Self::start_keepalive(&dev).ok();
        client.Start()?;

        Ok(Loopback { id, client, capture, conv, keepalive })
    }

    unsafe fn start_keepalive(dev: &IMMDevice) -> windows::core::Result<(IAudioClient, IAudioRenderClient, u32)> {
        let client: IAudioClient = dev.Activate(CLSCTX_ALL, None)?;
        let mix = client.GetMixFormat()?;
        let r = client.Initialize(AUDCLNT_SHAREMODE_SHARED, 0, HNS_PER_SEC / 5, 0, mix, None);
        CoTaskMemFree(Some(mix as _));
        r?;
        let size = client.GetBufferSize()?;
        let render: IAudioRenderClient = client.GetService()?;
        render.GetBuffer(size)?;
        render.ReleaseBuffer(size, BUFFER_SILENT)?;
        client.Start()?;
        Ok((client, render, size))
    }

    fn drain(&mut self, out: &mut Vec<i16>) -> windows::core::Result<()> {
        unsafe {
            if let Some((client, render, size)) = &self.keepalive {
                if let Ok(pad) = client.GetCurrentPadding() {
                    let free = size - pad;
                    if free > 0 && render.GetBuffer(free).is_ok() {
                        let _ = render.ReleaseBuffer(free, BUFFER_SILENT);
                    }
                }
            }
            loop {
                if self.capture.GetNextPacketSize()? == 0 {
                    return Ok(());
                }
                let mut data = std::ptr::null_mut();
                let mut n = 0u32;
                let mut flags = 0u32;
                self.capture.GetBuffer(&mut data, &mut n, &mut flags, None, None)?;
                if flags & BUFFER_SILENT != 0 || data.is_null() {
                    self.conv.push_silence(n as usize, out);
                } else {
                    self.conv.push(data, n as usize, out);
                }
                self.capture.ReleaseBuffer(n)?;
            }
        }
    }
}

impl Drop for Loopback {
    fn drop(&mut self) {
        unsafe {
            let _ = self.client.Stop();
            if let Some((c, _, _)) = &self.keepalive {
                let _ = c.Stop();
            }
        }
    }
}

fn float_stereo_48k() -> WAVEFORMATEXTENSIBLE {
    let mut w: WAVEFORMATEXTENSIBLE = unsafe { std::mem::zeroed() };
    w.Format.wFormatTag = WAVE_FORMAT_EXTENSIBLE_TAG;
    w.Format.nChannels = 2;
    w.Format.nSamplesPerSec = SAMPLE_RATE;
    w.Format.wBitsPerSample = 32;
    w.Format.nBlockAlign = 8;
    w.Format.nAvgBytesPerSec = SAMPLE_RATE * 8;
    w.Format.cbSize = 22;
    w.Samples.wValidBitsPerSample = 32;
    w.dwChannelMask = 0x3; // front left | front right
    w.SubFormat = SUBTYPE_FLOAT;
    w
}

// ----------------------------------------------------------- conversion ----

#[derive(Clone, Copy)]
enum SampleKind {
    F32,
    I16,
    I24,
    I32,
}

/// Converts any device format to 48 kHz stereo 16-bit.
struct Converter {
    kind: SampleKind,
    channels: usize,
    rate: u32,
    // Linear resampler state.
    step: f64,
    pos: f64,
    prev: [f32; 2],
}

impl Converter {
    fn new(kind: SampleKind, channels: usize, rate: u32) -> Converter {
        Converter {
            kind,
            channels: channels.max(1),
            rate,
            step: rate as f64 / SAMPLE_RATE as f64,
            pos: 0.0,
            prev: [0.0; 2],
        }
    }

    unsafe fn from_format(f: *const WAVEFORMATEX) -> Option<Converter> {
        let f = &*f;
        let mut tag = f.wFormatTag;
        if tag == WAVE_FORMAT_EXTENSIBLE_TAG {
            let x = &*(f as *const WAVEFORMATEX as *const WAVEFORMATEXTENSIBLE);
            let sub = x.SubFormat;
            tag = if sub == SUBTYPE_FLOAT {
                WAVE_FORMAT_IEEE_FLOAT_TAG
            } else if sub == SUBTYPE_PCM {
                WAVE_FORMAT_PCM_TAG
            } else {
                0
            };
        }
        let kind = match (tag, f.wBitsPerSample) {
            (WAVE_FORMAT_IEEE_FLOAT_TAG, 32) => SampleKind::F32,
            (WAVE_FORMAT_PCM_TAG, 16) => SampleKind::I16,
            (WAVE_FORMAT_PCM_TAG, 24) => SampleKind::I24,
            (WAVE_FORMAT_PCM_TAG, 32) => SampleKind::I32,
            _ => return None,
        };
        Some(Converter::new(kind, f.nChannels as usize, f.nSamplesPerSec))
    }

    fn push_silence(&mut self, frames: usize, out: &mut Vec<i16>) {
        for _ in 0..frames {
            self.frame([0.0, 0.0], out);
        }
    }

    unsafe fn push(&mut self, data: *const u8, frames: usize, out: &mut Vec<i16>) {
        let ch = self.channels;
        let kind = self.kind;
        let sample = |i: usize| -> f32 {
            match kind {
                SampleKind::F32 => *(data as *const f32).add(i),
                SampleKind::I16 => *(data as *const i16).add(i) as f32 / 32768.0,
                SampleKind::I32 => *(data as *const i32).add(i) as f32 / 2147483648.0,
                SampleKind::I24 => {
                    let p = data.add(i * 3);
                    let v = ((*p as i32) << 8 | (*p.add(1) as i32) << 16 | (*p.add(2) as i32) << 24) >> 8;
                    v as f32 / 8388608.0
                }
            }
        };
        for f in 0..frames {
            let b = f * ch;
            let lr = match ch {
                1 => {
                    let m = sample(b);
                    [m, m]
                }
                2 => [sample(b), sample(b + 1)],
                _ => {
                    // Standard order: FL FR FC LFE BL BR SL SR
                    const K: f32 = 0.7071;
                    let mut l = sample(b);
                    let mut r = sample(b + 1);
                    let c = sample(b + 2) * K;
                    l += c;
                    r += c;
                    let mut i = 4;
                    while i + 1 < ch.min(8) {
                        l += sample(b + i) * K;
                        r += sample(b + i + 1) * K;
                        i += 2;
                    }
                    [l, r]
                }
            };
            self.frame(lr, out);
        }
    }

    #[inline]
    fn frame(&mut self, cur: [f32; 2], out: &mut Vec<i16>) {
        if self.rate == SAMPLE_RATE {
            out.push(to_i16(cur[0]));
            out.push(to_i16(cur[1]));
            return;
        }
        // Emit output samples that fall between `prev` (t=0) and `cur` (t=1).
        while self.pos < 1.0 {
            let t = self.pos as f32;
            out.push(to_i16(self.prev[0] + (cur[0] - self.prev[0]) * t));
            out.push(to_i16(self.prev[1] + (cur[1] - self.prev[1]) * t));
            self.pos += self.step;
        }
        self.pos -= 1.0;
        self.prev = cur;
    }
}

#[inline]
fn to_i16(v: f32) -> i16 {
    (v * 32767.0).round().clamp(-32768.0, 32767.0) as i16
}
