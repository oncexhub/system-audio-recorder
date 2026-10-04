//! Output files. Both writers take interleaved 16-bit stereo PCM at 48 kHz
//! and stream it straight to disk, so memory use stays flat no matter how
//! long a recording runs.

use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::Path;
use std::time::{Duration, Instant};

use windows::core::HSTRING;
use windows::Win32::Media::MediaFoundation::*;

pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: u32 = 2;
const BLOCK_ALIGN: u32 = CHANNELS * 2;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Wav,
    Mp3,
}

impl Format {
    pub fn ext(self) -> &'static str {
        match self {
            Format::Wav => "wav",
            Format::Mp3 => "mp3",
        }
    }
}

pub enum Sink {
    Wav(WavWriter),
    Mp3(Mp3Writer),
}

impl Sink {
    pub fn create(path: &Path, format: Format) -> Result<Sink, String> {
        match format {
            Format::Wav => WavWriter::create(path).map(Sink::Wav),
            Format::Mp3 => Mp3Writer::create(path).map(Sink::Mp3),
        }
    }

    pub fn write(&mut self, samples: &[i16]) -> Result<(), String> {
        match self {
            Sink::Wav(w) => w.write(samples),
            Sink::Mp3(w) => w.write(samples),
        }
    }

    pub fn finish(self) -> Result<(), String> {
        match self {
            Sink::Wav(w) => w.finish(),
            Sink::Mp3(w) => w.finish(),
        }
    }
}

// ---------------------------------------------------------------- WAV ----

/// WAV writer that reserves room for an RF64 `ds64` chunk, so recordings
/// larger than 4 GB are upgraded to RF64 instead of being corrupted. The
/// header is refreshed every few seconds so the file stays playable even if
/// the PC loses power mid-recording.
pub struct WavWriter {
    out: BufWriter<File>,
    data_bytes: u64,
    last_header: Instant,
}

const WAV_HEADER_LEN: u64 = 80;

impl WavWriter {
    fn create(path: &Path) -> Result<Self, String> {
        let file = File::create(path).map_err(|e| format!("Could not create file: {e}"))?;
        let mut w = WavWriter {
            out: BufWriter::with_capacity(1 << 20, file),
            data_bytes: 0,
            last_header: Instant::now(),
        };
        w.out.write_all(&w.header()).map_err(io_err)?;
        Ok(w)
    }

    fn header(&self) -> [u8; WAV_HEADER_LEN as usize] {
        let mut h = [0u8; WAV_HEADER_LEN as usize];
        let riff_size = self.data_bytes + WAV_HEADER_LEN - 8;
        let rf64 = riff_size > u32::MAX as u64;
        let mut p = 0;
        let mut put = |b: &[u8]| {
            h[p..p + b.len()].copy_from_slice(b);
            p += b.len();
        };
        if rf64 {
            put(b"RF64");
            put(&u32::MAX.to_le_bytes());
            put(b"WAVE");
            put(b"ds64");
            put(&28u32.to_le_bytes());
            put(&riff_size.to_le_bytes());
            put(&self.data_bytes.to_le_bytes());
            put(&(self.data_bytes / BLOCK_ALIGN as u64).to_le_bytes());
            put(&0u32.to_le_bytes());
        } else {
            put(b"RIFF");
            put(&(riff_size as u32).to_le_bytes());
            put(b"WAVE");
            put(b"JUNK");
            put(&28u32.to_le_bytes());
            put(&[0u8; 28]);
        }
        put(b"fmt ");
        put(&16u32.to_le_bytes());
        put(&1u16.to_le_bytes()); // PCM
        put(&(CHANNELS as u16).to_le_bytes());
        put(&SAMPLE_RATE.to_le_bytes());
        put(&(SAMPLE_RATE * BLOCK_ALIGN).to_le_bytes());
        put(&(BLOCK_ALIGN as u16).to_le_bytes());
        put(&16u16.to_le_bytes());
        put(b"data");
        let data_field = if rf64 { u32::MAX } else { self.data_bytes as u32 };
        put(&data_field.to_le_bytes());
        h
    }

    fn write_header(&mut self) -> Result<(), String> {
        let h = self.header();
        self.out.flush().map_err(io_err)?;
        let f = self.out.get_mut();
        f.seek(SeekFrom::Start(0)).map_err(io_err)?;
        f.write_all(&h).map_err(io_err)?;
        f.seek(SeekFrom::End(0)).map_err(io_err)?;
        self.last_header = Instant::now();
        Ok(())
    }

    fn write(&mut self, samples: &[i16]) -> Result<(), String> {
        let bytes: &[u8] = unsafe {
            std::slice::from_raw_parts(samples.as_ptr() as *const u8, samples.len() * 2)
        };
        self.out.write_all(bytes).map_err(io_err)?;
        self.data_bytes += bytes.len() as u64;
        if self.last_header.elapsed() > Duration::from_secs(5) {
            self.write_header()?;
        }
        Ok(())
    }

    fn finish(mut self) -> Result<(), String> {
        self.write_header()?;
        self.out.get_mut().sync_all().map_err(io_err)
    }
}

fn io_err(e: std::io::Error) -> String {
    format!("Could not write to disk: {e}")
}

// ---------------------------------------------------------------- MP3 ----

/// MP3 (320 kbps) through the encoder that ships with Windows (Media
/// Foundation), so no extra libraries are needed.
pub struct Mp3Writer {
    writer: IMFSinkWriter,
    stream: u32,
    pending: Vec<i16>,
    frames_done: u64,
}

const MP3_CHUNK_FRAMES: usize = (SAMPLE_RATE / 10) as usize; // 100 ms per sample

impl Mp3Writer {
    fn create(path: &Path) -> Result<Self, String> {
        unsafe { Self::create_inner(path) }.map_err(|e| format!("MP3 encoder error: {}", e.message()))
    }

    unsafe fn create_inner(path: &Path) -> windows::core::Result<Self> {
        MFStartup(MF_VERSION, MFSTARTUP_LITE)?;

        let mut attrs: Option<IMFAttributes> = None;
        MFCreateAttributes(&mut attrs, 1)?;
        let attrs = attrs.unwrap();
        attrs.SetGUID(&MF_TRANSCODE_CONTAINERTYPE, &MFTranscodeContainerType_MP3)?;

        let url = HSTRING::from(path.as_os_str());
        let writer = MFCreateSinkWriterFromURL(&url, None, &attrs)?;

        let out = MFCreateMediaType()?;
        out.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        out.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_MP3)?;
        out.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, SAMPLE_RATE)?;
        out.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, CHANNELS)?;
        out.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 320_000 / 8)?;
        let stream = writer.AddStream(&out)?;

        let inp = MFCreateMediaType()?;
        inp.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
        inp.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
        inp.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, SAMPLE_RATE)?;
        inp.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, CHANNELS)?;
        inp.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
        inp.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, BLOCK_ALIGN)?;
        inp.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, SAMPLE_RATE * BLOCK_ALIGN)?;
        writer.SetInputMediaType(stream, &inp, None)?;
        writer.BeginWriting()?;

        Ok(Mp3Writer {
            writer,
            stream,
            pending: Vec::with_capacity(MP3_CHUNK_FRAMES * 4),
            frames_done: 0,
        })
    }

    fn write(&mut self, samples: &[i16]) -> Result<(), String> {
        self.pending.extend_from_slice(samples);
        if self.pending.len() >= MP3_CHUNK_FRAMES * CHANNELS as usize {
            self.flush()?;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), String> {
        if self.pending.is_empty() {
            return Ok(());
        }
        unsafe { self.flush_inner() }.map_err(|e| format!("MP3 encoder error: {}", e.message()))?;
        self.pending.clear();
        Ok(())
    }

    unsafe fn flush_inner(&mut self) -> windows::core::Result<()> {
        let len = (self.pending.len() * 2) as u32;
        let frames = (self.pending.len() / CHANNELS as usize) as u64;
        let buf = MFCreateMemoryBuffer(len)?;
        let mut ptr = std::ptr::null_mut();
        buf.Lock(&mut ptr, None, None)?;
        std::ptr::copy_nonoverlapping(self.pending.as_ptr() as *const u8, ptr, len as usize);
        buf.Unlock()?;
        buf.SetCurrentLength(len)?;

        let sample = MFCreateSample()?;
        sample.AddBuffer(&buf)?;
        let to_hns = |f: u64| (f * 10_000_000 / SAMPLE_RATE as u64) as i64;
        sample.SetSampleTime(to_hns(self.frames_done))?;
        sample.SetSampleDuration(to_hns(self.frames_done + frames) - to_hns(self.frames_done))?;
        self.writer.WriteSample(self.stream, &sample)?;
        self.frames_done += frames;
        Ok(())
    }

    fn finish(mut self) -> Result<(), String> {
        let r = self.flush();
        let fin = unsafe { self.writer.Finalize() };
        drop(self.writer);
        unsafe {
            let _ = MFShutdown();
        }
        r?;
        fin.map_err(|e| format!("Could not finalize MP3: {}", e.message()))
    }
}
