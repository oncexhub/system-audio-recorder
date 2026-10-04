//! Simple trim editor: load a recording's waveform, preview it, and cut off
//! the start and/or end. Cutting never re-encodes: WAV is copied byte-exact
//! and MP3/M4A are cut on frame boundaries (~21-24 ms), so quality never drops and
//! even hours-long files save in seconds.

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use windows::core::HSTRING;
use windows::Win32::Media::Audio::*;
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::*;
use windows::Win32::System::Variant::VT_I8;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Wav,
    Mp3,
    M4a,
}

impl Kind {
    pub fn of(path: &Path) -> Option<Kind> {
        match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "wav" => Some(Kind::Wav),
            "mp3" => Some(Kind::Mp3),
            "m4a" => Some(Kind::M4a),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Handle {
    Start,
    End,
}

/// Shared waveform data filled in by the loader thread.
#[derive(Default)]
struct Wave {
    peaks: Vec<f32>,
    block_secs: f64,
    estimated: f64,
    duration: Option<f64>,
    failed: Option<String>,
}

pub struct Editor {
    pub path: PathBuf,
    pub kind: Kind,
    pub start: f64,
    pub end: f64,
    pub playhead: f64,
    pub active: Handle,
    wave: Arc<Mutex<Wave>>,
    cancel: Arc<AtomicBool>,
    loader: Option<JoinHandle<()>>,
    player: Option<Player>,
    end_is_default: bool,
    /// Visible part of the waveform (seconds); `None` = whole file.
    view: Option<(f64, f64)>,
}

/// Narrowest zoom: this many seconds across the waveform.
const MIN_VIEW: f64 = 0.25;

impl Editor {
    pub fn open(path: PathBuf) -> Result<Editor, String> {
        let kind = Kind::of(&path).ok_or("Only WAV, MP3 and M4A files can be trimmed.")?;
        if !path.exists() {
            return Err("That file no longer exists.".into());
        }
        let wave = Arc::new(Mutex::new(Wave::default()));
        let cancel = Arc::new(AtomicBool::new(false));
        let (w, c, p) = (wave.clone(), cancel.clone(), path.clone());
        let loader = std::thread::spawn(move || load_wave(&p, &w, &c));

        // Wait briefly for the duration so the editor opens fully formed.
        for _ in 0..200 {
            if let Ok(w) = wave.lock() {
                if w.estimated > 0.0 || w.failed.is_some() {
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let (est, failed) = wave.lock().map(|w| (w.estimated, w.failed.clone())).unwrap_or((0.0, None));
        if let Some(e) = failed {
            return Err(e);
        }
        Ok(Editor {
            path,
            kind,
            start: 0.0,
            end: est,
            playhead: 0.0,
            active: Handle::Start,
            wave,
            cancel,
            loader: Some(loader),
            player: None,
            end_is_default: true,
            view: None,
        })
    }

    pub fn duration(&self) -> f64 {
        self.wave.lock().map(|w| w.duration.unwrap_or(w.estimated)).unwrap_or(0.0)
    }

    /// Loading progress 0..1, or None when finished.
    pub fn loading(&self) -> Option<f32> {
        let w = self.wave.lock().ok()?;
        if w.duration.is_some() || w.failed.is_some() {
            return None;
        }
        let done = w.peaks.len() as f64 * w.block_secs;
        Some((done / w.estimated.max(0.001)).clamp(0.0, 1.0) as f32)
    }

    pub fn load_error(&self) -> Option<String> {
        self.wave.lock().ok()?.failed.clone()
    }

    /// Call regularly: picks up the exact duration once loading finishes.
    pub fn update(&mut self) {
        let d = self.duration();
        if self.end_is_default || self.end > d {
            self.end = d;
        }
        self.start = self.start.min(self.end);
        if let Some(p) = &self.player {
            if p.finished() {
                self.player = None;
                self.playhead = self.start;
            } else {
                self.playhead = p.position();
            }
        }
    }

    /// Peak level per display column (0..1) for the time range `from..to`.
    pub fn columns(&self, n: usize, from: f64, to: f64) -> Vec<f32> {
        let mut out = vec![0.0; n];
        let Ok(w) = self.wave.lock() else { return out };
        if to <= from || w.block_secs <= 0.0 || n == 0 {
            return out;
        }
        let per_col = (to - from) / n as f64;
        for (c, o) in out.iter_mut().enumerate() {
            let a = ((from + c as f64 * per_col) / w.block_secs) as usize;
            let b = (((from + (c + 1) as f64 * per_col) / w.block_secs).ceil() as usize).max(a + 1);
            let mut m = 0.0f32;
            for i in a..b.min(w.peaks.len()) {
                m = m.max(w.peaks[i]);
            }
            *o = m;
        }
        out
    }

    /// Visible time range of the waveform.
    pub fn view(&self) -> (f64, f64) {
        let d = self.duration();
        match self.view {
            Some((a, b)) => (a.max(0.0), b.min(d)),
            None => (0.0, d),
        }
    }

    pub fn is_zoomed(&self) -> bool {
        self.view.is_some()
    }

    /// Zooms in (factor > 1) or out (< 1), keeping time `at` in place.
    pub fn zoom(&mut self, factor: f64, at: f64) {
        let d = self.duration();
        let (a, b) = self.view();
        let span = ((b - a) / factor).clamp(MIN_VIEW.min(d), d);
        if span >= d - 1e-9 {
            self.view = None;
            return;
        }
        let rel = if b > a { (at - a) / (b - a) } else { 0.5 };
        let na = at - rel * span;
        self.set_view(na, na + span);
    }

    /// Moves the view by `by` seconds.
    pub fn pan(&mut self, by: f64) {
        if let Some((a, b)) = self.view {
            self.set_view(a + by, b + by);
        }
    }

    /// Centers the view on `t` (keeps the zoom level).
    pub fn center_on(&mut self, t: f64) {
        if let Some((a, b)) = self.view {
            let half = (b - a) / 2.0;
            self.set_view(t - half, t + half);
        }
    }

    pub fn fit(&mut self) {
        self.view = None;
    }

    fn set_view(&mut self, a: f64, b: f64) {
        let d = self.duration();
        let span = (b - a).min(d);
        let a = a.clamp(0.0, (d - span).max(0.0));
        self.view = Some((a, a + span));
    }

    /// While playing, page the view along so the playhead stays visible.
    pub fn follow_playhead(&mut self) {
        if let (Some((a, b)), true) = (self.view, self.is_playing()) {
            if self.playhead > b || self.playhead < a {
                let span = b - a;
                self.set_view(self.playhead - span * 0.1, self.playhead + span * 0.9);
            }
        }
    }

    pub fn set_handle(&mut self, h: Handle, t: f64) {
        let dur = self.duration();
        const MIN_LEN: f64 = 0.05;
        match h {
            Handle::Start => self.start = t.clamp(0.0, (self.end - MIN_LEN).max(0.0)),
            Handle::End => {
                self.end = t.clamp((self.start + MIN_LEN).min(dur), dur);
                self.end_is_default = false;
            }
        }
        self.active = h;
    }

    pub fn nudge(&mut self, h: Handle, by: f64) {
        let t = if h == Handle::Start { self.start } else { self.end };
        self.set_handle(h, t + by);
    }

    pub fn is_playing(&self) -> bool {
        self.player.is_some()
    }

    pub fn stop(&mut self) {
        if let Some(p) = self.player.take() {
            self.playhead = p.position();
            p.stop();
        }
    }

    pub fn play(&mut self, from: f64, to: f64) -> Result<(), String> {
        self.stop();
        let from = from.clamp(0.0, self.duration());
        self.playhead = from;
        self.player = Some(Player::start(&self.path, from, to)?);
        Ok(())
    }

    /// Play/pause the selection from the playhead.
    pub fn toggle_play(&mut self) -> Result<(), String> {
        if self.is_playing() {
            self.stop();
            return Ok(());
        }
        let from = if self.playhead >= self.start && self.playhead < self.end - 0.05 { self.playhead } else { self.start };
        self.play(from, self.end)
    }

    /// Stops playback and the loader so the file can be replaced.
    pub fn release(&mut self) {
        self.stop();
        self.cancel.store(true, Ordering::SeqCst);
        if let Some(l) = self.loader.take() {
            let _ = l.join();
        }
    }

    pub fn save(&mut self, dest: &Path) -> Result<(), String> {
        match self.kind {
            Kind::Wav => trim_wav(&self.path, dest, self.start, self.end),
            Kind::Mp3 => trim_mp3(&self.path, dest, self.start, self.end),
            Kind::M4a => crate::mp4::trim_m4a(&self.path, dest, self.start, self.end),
        }
    }
}

impl Drop for Editor {
    fn drop(&mut self) {
        self.release();
    }
}

// --------------------------------------------------------- decoding ----

struct Reader {
    reader: IMFSourceReader,
    rate: u32,
    channels: u32,
}

const AUDIO: u32 = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;

/// Opens any WAV/MP3 file and decodes it to 16-bit PCM.
unsafe fn open_reader(path: &Path) -> windows::core::Result<Reader> {
    let reader = MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), None)?;
    reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)?;
    reader.SetStreamSelection(AUDIO, true)?;
    let mt = MFCreateMediaType()?;
    mt.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
    mt.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
    mt.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
    reader.SetCurrentMediaType(AUDIO, None, &mt)?;
    let cur = reader.GetCurrentMediaType(AUDIO)?;
    let rate = cur.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND)?;
    let channels = cur.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS)?.max(1);
    Ok(Reader { reader, rate, channels })
}

unsafe fn duration_of(r: &IMFSourceReader) -> f64 {
    match r.GetPresentationAttribute(MF_SOURCE_READER_MEDIASOURCE.0 as u32, &MF_PD_DURATION) {
        Ok(v) => v.Anonymous.Anonymous.Anonymous.uhVal as f64 / 1e7,
        Err(_) => 0.0,
    }
}

/// Reads the next block of PCM samples; None at end of stream.
unsafe fn read_block(r: &IMFSourceReader, out: &mut Vec<i16>) -> windows::core::Result<Option<i64>> {
    let mut flags = 0u32;
    let mut ts = 0i64;
    let mut sample: Option<IMFSample> = None;
    r.ReadSample(AUDIO, 0, None, Some(&mut flags), Some(&mut ts), Some(&mut sample))?;
    if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
        return Ok(None);
    }
    out.clear();
    if let Some(s) = sample {
        let buf = s.ConvertToContiguousBuffer()?;
        let mut p = std::ptr::null_mut();
        let mut len = 0u32;
        buf.Lock(&mut p, None, Some(&mut len))?;
        out.extend_from_slice(std::slice::from_raw_parts(p as *const i16, len as usize / 2));
        buf.Unlock()?;
    }
    Ok(Some(ts))
}

struct MfSession;

impl MfSession {
    fn start() -> MfSession {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let _ = MFStartup(MF_VERSION, MFSTARTUP_LITE);
        }
        MfSession
    }
}

impl Drop for MfSession {
    fn drop(&mut self) {
        unsafe {
            let _ = MFShutdown();
            CoUninitialize();
        }
    }
}

fn load_wave(path: &Path, wave: &Mutex<Wave>, cancel: &AtomicBool) {
    let _mf = MfSession::start();
    let fail = |e: String| {
        if let Ok(mut w) = wave.lock() {
            w.failed = Some(e);
        }
    };
    let r = match unsafe { open_reader(path) } {
        Ok(r) => r,
        Err(e) => return fail(format!("Could not open this file: {}", e.message())),
    };
    let est = unsafe { duration_of(&r.reader) }.max(0.001);
    // Fine enough for precise trimming, small enough for hours-long files.
    let block_secs = (est / 400_000.0).max(0.002);
    let block = ((block_secs * r.rate as f64) as usize).max(1);
    if let Ok(mut w) = wave.lock() {
        w.estimated = est;
        w.block_secs = block as f64 / r.rate as f64;
    }

    let ch = r.channels as usize;
    let mut buf = Vec::new();
    let mut peak = 0i32;
    let mut count = 0usize;
    let mut frames: u64 = 0;
    let mut new = Vec::new();
    while !cancel.load(Ordering::Relaxed) {
        match unsafe { read_block(&r.reader, &mut buf) } {
            Ok(Some(_)) => {}
            Ok(None) => break,
            Err(e) => return fail(format!("Could not read this file: {}", e.message())),
        }
        for f in buf.chunks_exact(ch) {
            for &s in f {
                peak = peak.max((s as i32).abs());
            }
            count += 1;
            if count == block {
                new.push(peak as f32 / 32768.0);
                peak = 0;
                count = 0;
            }
        }
        frames += (buf.len() / ch) as u64;
        if !new.is_empty() {
            if let Ok(mut w) = wave.lock() {
                w.peaks.extend(new.drain(..));
            }
        }
    }
    if cancel.load(Ordering::Relaxed) {
        return;
    }
    if count > 0 {
        new.push(peak as f32 / 32768.0);
    }
    if let Ok(mut w) = wave.lock() {
        w.peaks.extend(new.drain(..));
        w.duration = Some(frames as f64 / r.rate as f64);
    }
}

// --------------------------------------------------------- playback ----

struct Player {
    stop: Arc<AtomicBool>,
    done: Arc<AtomicBool>,
    pos: Arc<AtomicU64>, // f64 bits, seconds
    thread: Option<JoinHandle<()>>,
}

impl Player {
    fn start(path: &Path, from: f64, to: f64) -> Result<Player, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let done = Arc::new(AtomicBool::new(false));
        let pos = Arc::new(AtomicU64::new(from.to_bits()));
        let (s, d, p, path) = (stop.clone(), done.clone(), pos.clone(), path.to_path_buf());
        let thread = std::thread::spawn(move || {
            let _mf = MfSession::start();
            let _ = unsafe { play_range(&path, from, to, &s, &p) };
            d.store(true, Ordering::SeqCst);
        });
        Ok(Player { stop, done, pos, thread: Some(thread) })
    }

    fn position(&self) -> f64 {
        f64::from_bits(self.pos.load(Ordering::Relaxed))
    }

    fn finished(&self) -> bool {
        self.done.load(Ordering::SeqCst)
    }

    fn stop(mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

unsafe fn play_range(path: &Path, from: f64, to: f64, stop: &AtomicBool, pos: &AtomicU64) -> windows::core::Result<()> {
    let r = open_reader(path)?;
    let ch = r.channels as usize;
    let rate = r.rate;

    let mut var = PROPVARIANT::default();
    (*var.Anonymous.Anonymous).vt = VT_I8;
    (*var.Anonymous.Anonymous).Anonymous.hVal = (from * 1e7) as i64;
    r.reader.SetCurrentPosition(&windows::core::GUID::zeroed(), &var)?;

    let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
    let dev = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
    let client: IAudioClient = dev.Activate(CLSCTX_ALL, None)?;
    let fmt = WAVEFORMATEX {
        wFormatTag: 1,
        nChannels: ch as u16,
        nSamplesPerSec: rate,
        nAvgBytesPerSec: rate * ch as u32 * 2,
        nBlockAlign: ch as u16 * 2,
        wBitsPerSample: 16,
        cbSize: 0,
    };
    const AUTOCONVERTPCM: u32 = 0x8000_0000;
    const SRC_DEFAULT_QUALITY: u32 = 0x0800_0000;
    client.Initialize(AUDCLNT_SHAREMODE_SHARED, AUTOCONVERTPCM | SRC_DEFAULT_QUALITY, 2_000_000, 0, &fmt, None)?;
    let size = client.GetBufferSize()?;
    let render: IAudioRenderClient = client.GetService()?;

    let total = (((to - from).max(0.0)) * rate as f64) as u64;
    let mut pending: std::collections::VecDeque<i16> = std::collections::VecDeque::new();
    let mut block = Vec::new();
    let mut eof = false;
    let mut queued: u64 = 0; // frames handed to the device
    let mut started = false;

    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        // Decode ahead.
        while !eof && pending.len() / ch < rate as usize / 2 && queued + ((pending.len() / ch) as u64) < total {
            match read_block(&r.reader, &mut block)? {
                None => eof = true,
                Some(ts) => {
                    // Seeking lands slightly early; drop audio before `from`.
                    let t0 = ts as f64 / 1e7;
                    let skip = if t0 < from { (((from - t0) * rate as f64) as usize * ch).min(block.len()) } else { 0 };
                    pending.extend(&block[skip..]);
                }
            }
        }
        let pad = client.GetCurrentPadding()?;
        let free = (size - pad) as u64;
        let left = total.saturating_sub(queued);
        let n = free.min(left).min((pending.len() / ch) as u64) as u32;
        if n > 0 {
            let p = render.GetBuffer(n)? as *mut i16;
            for i in 0..(n as usize * ch) {
                *p.add(i) = pending.pop_front().unwrap_or(0);
            }
            render.ReleaseBuffer(n, 0)?;
            queued += n as u64;
        }
        if !started {
            client.Start()?;
            started = true;
        }
        let played = queued.saturating_sub(client.GetCurrentPadding()? as u64);
        pos.store((from + played as f64 / rate as f64).to_bits(), Ordering::Relaxed);
        let finished_feeding = queued >= total || (eof && pending.is_empty());
        if finished_feeding && client.GetCurrentPadding()? == 0 {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let _ = client.Stop();
    Ok(())
}

// ---------------------------------------------------------- trimming ----

fn io<E: std::fmt::Display>(e: E) -> String {
    format!("Could not save: {e}")
}

fn trim_wav(src: &Path, dst: &Path, start: f64, end: f64) -> Result<(), String> {
    let mut f = BufReader::new(File::open(src).map_err(io)?);
    let file_len = f.get_ref().metadata().map_err(io)?.len();
    let mut hdr = [0u8; 12];
    f.read_exact(&mut hdr).map_err(io)?;
    let rf64 = &hdr[0..4] == b"RF64";
    if !(&hdr[0..4] == b"RIFF" || rf64) || &hdr[8..12] != b"WAVE" {
        return Err("This WAV file has an unsupported layout.".into());
    }
    let mut fmt: Option<Vec<u8>> = None;
    let mut data64: Option<u64> = None;
    let (data_off, data_len) = loop {
        let mut ch = [0u8; 8];
        if f.read_exact(&mut ch).is_err() {
            return Err("This WAV file has no audio data.".into());
        }
        let size = u32::from_le_bytes(ch[4..8].try_into().unwrap()) as u64;
        let pos = f.stream_position().map_err(io)?;
        match &ch[0..4] {
            b"fmt " => {
                let mut b = vec![0u8; size as usize];
                f.read_exact(&mut b).map_err(io)?;
                fmt = Some(b);
            }
            b"ds64" => {
                let mut b = [0u8; 16];
                f.read_exact(&mut b).map_err(io)?;
                data64 = Some(u64::from_le_bytes(b[8..16].try_into().unwrap()));
            }
            b"data" => {
                let len = if rf64 && size == 0xFFFF_FFFF { data64.unwrap_or(file_len - pos) } else { size };
                break (pos, len.min(file_len - pos));
            }
            _ => {}
        }
        f.seek(SeekFrom::Start(pos + size + (size & 1))).map_err(io)?;
    };
    let fmt = fmt.ok_or("This WAV file has no format information.")?;
    if fmt.len() < 16 {
        return Err("This WAV file has an unsupported format.".into());
    }
    let rate = u32::from_le_bytes(fmt[4..8].try_into().unwrap()) as f64;
    let align = u16::from_le_bytes(fmt[12..14].try_into().unwrap()).max(1) as u64;
    let total_frames = data_len / align;
    let a = ((start * rate) as u64).min(total_frames);
    let b = ((end * rate).round() as u64).clamp(a, total_frames);
    let bytes = (b - a) * align;

    let mut out = BufWriter::with_capacity(1 << 20, File::create(dst).map_err(io)?);
    let fmt_padded = fmt.len() as u64 + (fmt.len() as u64 & 1);
    let riff = 4 + 36 + 8 + fmt_padded + 8 + bytes;
    let big = riff > u32::MAX as u64;
    out.write_all(if big { b"RF64" } else { b"RIFF" }).map_err(io)?;
    out.write_all(&(if big { u32::MAX } else { riff as u32 }).to_le_bytes()).map_err(io)?;
    out.write_all(b"WAVE").map_err(io)?;
    if big {
        out.write_all(b"ds64").map_err(io)?;
        out.write_all(&28u32.to_le_bytes()).map_err(io)?;
        out.write_all(&riff.to_le_bytes()).map_err(io)?;
        out.write_all(&bytes.to_le_bytes()).map_err(io)?;
        out.write_all(&(b - a).to_le_bytes()).map_err(io)?;
        out.write_all(&0u32.to_le_bytes()).map_err(io)?;
    } else {
        out.write_all(b"JUNK").map_err(io)?;
        out.write_all(&28u32.to_le_bytes()).map_err(io)?;
        out.write_all(&[0u8; 28]).map_err(io)?;
    }
    out.write_all(b"fmt ").map_err(io)?;
    out.write_all(&(fmt.len() as u32).to_le_bytes()).map_err(io)?;
    out.write_all(&fmt).map_err(io)?;
    if fmt.len() & 1 == 1 {
        out.write_all(&[0]).map_err(io)?;
    }
    out.write_all(b"data").map_err(io)?;
    out.write_all(&(if big { u32::MAX } else { bytes as u32 }).to_le_bytes()).map_err(io)?;

    f.seek(SeekFrom::Start(data_off + a * align)).map_err(io)?;
    std::io::copy(&mut f.take(bytes), &mut out).map_err(io)?;
    out.flush().map_err(io)?;
    out.get_ref().sync_all().map_err(io)
}

/// Parses an MPEG audio frame header: (frame length, samples per frame, sample rate).
fn mp3_frame(h: [u8; 4]) -> Option<(usize, u32, u32)> {
    if h[0] != 0xFF || h[1] & 0xE0 != 0xE0 {
        return None;
    }
    let version = (h[1] >> 3) & 3; // 3 = MPEG1, 2 = MPEG2, 0 = MPEG2.5
    let layer = (h[1] >> 1) & 3; // 1 = Layer III
    let br_idx = (h[2] >> 4) as usize;
    let sr_idx = ((h[2] >> 2) & 3) as usize;
    let pad = ((h[2] >> 1) & 1) as usize;
    if version == 1 || layer != 1 || br_idx == 0 || br_idx == 15 || sr_idx == 3 {
        return None;
    }
    const BR1: [u32; 15] = [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320];
    const BR2: [u32; 15] = [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160];
    const SR: [u32; 3] = [44100, 48000, 32000];
    let (br, sr, coef, spf) = match version {
        3 => (BR1[br_idx], SR[sr_idx], 144, 1152),
        2 => (BR2[br_idx], SR[sr_idx] / 2, 72, 576),
        _ => (BR2[br_idx], SR[sr_idx] / 4, 72, 576),
    };
    let len = (coef * br * 1000 / sr) as usize + pad;
    Some((len, spf, sr))
}

fn trim_mp3(src: &Path, dst: &Path, start: f64, end: f64) -> Result<(), String> {
    let mut f = BufReader::with_capacity(1 << 20, File::open(src).map_err(io)?);
    let mut out = BufWriter::with_capacity(1 << 20, File::create(dst).map_err(io)?);

    // Skip an ID3v2 tag if present.
    let mut id3 = [0u8; 10];
    f.read_exact(&mut id3).map_err(io)?;
    if &id3[0..3] == b"ID3" {
        let size = id3[6..10].iter().fold(0u64, |acc, &b| (acc << 7) | (b & 0x7F) as u64);
        f.seek(SeekFrom::Start(10 + size)).map_err(io)?;
    } else {
        f.seek(SeekFrom::Start(0)).map_err(io)?;
    }

    let mut time = 0.0f64;
    let mut first = true;
    let mut frame = Vec::with_capacity(2048);
    let mut written = 0usize;
    loop {
        let mut h = [0u8; 4];
        if f.read_exact(&mut h).is_err() {
            break;
        }
        let Some((len, spf, sr)) = mp3_frame(h) else {
            // Lost sync (or trailing tag): step one byte and look again.
            f.seek(SeekFrom::Current(-3)).map_err(io)?;
            continue;
        };
        frame.clear();
        frame.extend_from_slice(&h);
        frame.resize(len.max(4), 0);
        if f.read_exact(&mut frame[4..]).is_err() {
            break;
        }
        // The first frame may be a Xing/Info header with the old length: drop it.
        if first {
            first = false;
            let w = &frame[..frame.len().min(64)];
            if w.windows(4).any(|x| x == b"Xing" || x == b"Info" || x == b"VBRI") {
                continue;
            }
        }
        let dur = spf as f64 / sr as f64;
        if time + dur / 2.0 >= start && time + dur / 2.0 <= end {
            out.write_all(&frame).map_err(io)?;
            written += 1;
        }
        time += dur;
        if time > end {
            break;
        }
    }
    if written == 0 {
        return Err("Nothing left to save after trimming.".into());
    }
    out.flush().map_err(io)?;
    out.get_ref().sync_all().map_err(io)
}
