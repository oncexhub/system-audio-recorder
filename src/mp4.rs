//! Minimal M4A (MP4 audio) support for AAC, without re-encoding:
//! * `adts_to_m4a` wraps a raw ADTS stream (what we record to, because it
//!   survives crashes) into a proper .m4a when recording stops.
//! * `read_m4a` + `Mp4Writer` let the trim editor copy a range of frames.

use std::fs::File;
use std::io::{BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::Path;

fn io<E: std::fmt::Display>(e: E) -> String {
    format!("Could not save: {e}")
}

/// Writes AAC frames into an .m4a file. Frames go straight to disk; only
/// their sizes are kept in memory (4 bytes per ~21 ms).
pub struct Mp4Writer {
    out: BufWriter<File>,
    asc: Vec<u8>,
    sample_rate: u32,
    channels: u16,
    mdat_start: u64,
    sizes: Vec<u32>,
    bytes: u64,
}

const FRAME_SAMPLES: u32 = 1024;

impl Mp4Writer {
    /// `asc` is the AAC AudioSpecificConfig (usually 2 bytes).
    pub fn create(dst: &Path, asc: Vec<u8>, sample_rate: u32, channels: u16) -> Result<Mp4Writer, String> {
        let mut out = BufWriter::with_capacity(1 << 20, File::create(dst).map_err(io)?);
        let mut ftyp = Vec::new();
        ftyp.extend_from_slice(b"M4A ");
        ftyp.extend_from_slice(&0u32.to_be_bytes());
        for brand in [b"M4A ", b"mp42", b"isom"] {
            ftyp.extend_from_slice(brand);
        }
        let ftyp = boxed(b"ftyp", &ftyp);
        out.write_all(&ftyp).map_err(io)?;
        // mdat with a 64-bit size, patched in finish().
        out.write_all(&1u32.to_be_bytes()).map_err(io)?;
        out.write_all(b"mdat").map_err(io)?;
        out.write_all(&0u64.to_be_bytes()).map_err(io)?;
        Ok(Mp4Writer {
            out,
            asc,
            sample_rate,
            channels,
            mdat_start: ftyp.len() as u64,
            sizes: Vec::new(),
            bytes: 0,
        })
    }

    pub fn add_frame(&mut self, frame: &[u8]) -> Result<(), String> {
        self.out.write_all(frame).map_err(io)?;
        self.sizes.push(frame.len() as u32);
        self.bytes += frame.len() as u64;
        Ok(())
    }

    pub fn finish(mut self) -> Result<(), String> {
        if self.sizes.is_empty() {
            return Err("Nothing left to save.".into());
        }
        let moov = self.moov();
        self.out.write_all(&moov).map_err(io)?;
        self.out.flush().map_err(io)?;
        let f = self.out.get_mut();
        f.seek(SeekFrom::Start(self.mdat_start + 8)).map_err(io)?;
        f.write_all(&(16 + self.bytes).to_be_bytes()).map_err(io)?;
        f.sync_all().map_err(io)
    }

    fn moov(&self) -> Vec<u8> {
        let n = self.sizes.len() as u64;
        let duration = n * FRAME_SAMPLES as u64;
        let ts = self.sample_rate;
        let secs = duration as f64 / ts as f64;
        let avg_bitrate = if secs > 0.0 { (self.bytes as f64 * 8.0 / secs) as u32 } else { 0 };
        let max_size = self.sizes.iter().copied().max().unwrap_or(0);

        let mut mvhd = full(1, 0);
        mvhd.extend_from_slice(&[0u8; 16]); // creation + modification time
        mvhd.extend_from_slice(&ts.to_be_bytes());
        mvhd.extend_from_slice(&duration.to_be_bytes());
        mvhd.extend_from_slice(&0x0001_0000u32.to_be_bytes()); // rate 1.0
        mvhd.extend_from_slice(&0x0100u16.to_be_bytes()); // volume 1.0
        mvhd.extend_from_slice(&[0u8; 10]);
        mvhd.extend_from_slice(&MATRIX);
        mvhd.extend_from_slice(&[0u8; 24]);
        mvhd.extend_from_slice(&2u32.to_be_bytes()); // next track id

        let mut tkhd = full(1, 7);
        tkhd.extend_from_slice(&[0u8; 16]);
        tkhd.extend_from_slice(&1u32.to_be_bytes()); // track id
        tkhd.extend_from_slice(&[0u8; 4]);
        tkhd.extend_from_slice(&duration.to_be_bytes());
        tkhd.extend_from_slice(&[0u8; 8]);
        tkhd.extend_from_slice(&[0u8; 4]); // layer + alternate group
        tkhd.extend_from_slice(&0x0100u16.to_be_bytes());
        tkhd.extend_from_slice(&[0u8; 2]);
        tkhd.extend_from_slice(&MATRIX);
        tkhd.extend_from_slice(&[0u8; 8]); // width, height

        let mut mdhd = full(1, 0);
        mdhd.extend_from_slice(&[0u8; 16]);
        mdhd.extend_from_slice(&ts.to_be_bytes());
        mdhd.extend_from_slice(&duration.to_be_bytes());
        mdhd.extend_from_slice(&0x55C4u16.to_be_bytes()); // language "und"
        mdhd.extend_from_slice(&[0u8; 2]);

        let mut hdlr = full(0, 0);
        hdlr.extend_from_slice(&[0u8; 4]);
        hdlr.extend_from_slice(b"soun");
        hdlr.extend_from_slice(&[0u8; 12]);
        hdlr.extend_from_slice(b"SoundHandler\0");

        let mut smhd = full(0, 0);
        smhd.extend_from_slice(&[0u8; 4]);

        let mut dref = full(0, 0);
        dref.extend_from_slice(&1u32.to_be_bytes());
        dref.extend_from_slice(&boxed(b"url ", &full(0, 1)));
        let dinf = boxed(b"dinf", &boxed(b"dref", &dref));

        // esds: ES_Descriptor > DecoderConfigDescriptor > DecoderSpecificInfo (+ SLConfig)
        let mut dsi = vec![0x05, self.asc.len() as u8];
        dsi.extend_from_slice(&self.asc);
        let mut dcd = vec![0x40, 0x15]; // MPEG-4 audio, audio stream
        dcd.extend_from_slice(&max_size.to_be_bytes()[1..]); // buffer size (24 bit)
        dcd.extend_from_slice(&avg_bitrate.to_be_bytes()); // max bitrate
        dcd.extend_from_slice(&avg_bitrate.to_be_bytes()); // avg bitrate
        dcd.extend_from_slice(&dsi);
        let mut es = vec![0x00, 0x01, 0x00]; // ES_ID, flags
        es.extend_from_slice(&descriptor(0x04, &dcd));
        es.extend_from_slice(&descriptor(0x06, &[0x02]));
        let mut esds = full(0, 0);
        esds.extend_from_slice(&descriptor(0x03, &es));

        let mut mp4a = vec![0u8; 6];
        mp4a.extend_from_slice(&1u16.to_be_bytes()); // data reference index
        mp4a.extend_from_slice(&[0u8; 8]);
        mp4a.extend_from_slice(&self.channels.to_be_bytes());
        mp4a.extend_from_slice(&16u16.to_be_bytes());
        mp4a.extend_from_slice(&[0u8; 4]);
        mp4a.extend_from_slice(&((ts.min(65535)) << 16).to_be_bytes());
        mp4a.extend_from_slice(&boxed(b"esds", &esds));

        let mut stsd = full(0, 0);
        stsd.extend_from_slice(&1u32.to_be_bytes());
        stsd.extend_from_slice(&boxed(b"mp4a", &mp4a));

        let mut stts = full(0, 0);
        stts.extend_from_slice(&1u32.to_be_bytes());
        stts.extend_from_slice(&(n as u32).to_be_bytes());
        stts.extend_from_slice(&FRAME_SAMPLES.to_be_bytes());

        // All frames sit back to back in one chunk.
        let mut stsc = full(0, 0);
        stsc.extend_from_slice(&1u32.to_be_bytes());
        stsc.extend_from_slice(&1u32.to_be_bytes());
        stsc.extend_from_slice(&(n as u32).to_be_bytes());
        stsc.extend_from_slice(&1u32.to_be_bytes());

        let mut stsz = full(0, 0);
        stsz.extend_from_slice(&0u32.to_be_bytes());
        stsz.extend_from_slice(&(n as u32).to_be_bytes());
        for s in &self.sizes {
            stsz.extend_from_slice(&s.to_be_bytes());
        }

        let first = self.mdat_start + 16;
        let mut chunk = full(0, 0);
        chunk.extend_from_slice(&1u32.to_be_bytes());
        let co = if first > u32::MAX as u64 {
            chunk.extend_from_slice(&first.to_be_bytes());
            boxed(b"co64", &chunk)
        } else {
            chunk.extend_from_slice(&(first as u32).to_be_bytes());
            boxed(b"stco", &chunk)
        };

        let stbl = boxed(b"stbl", &[boxed(b"stsd", &stsd), boxed(b"stts", &stts), boxed(b"stsc", &stsc), boxed(b"stsz", &stsz), co].concat());
        let minf = boxed(b"minf", &[boxed(b"smhd", &smhd), dinf, stbl].concat());
        let mdia = boxed(b"mdia", &[boxed(b"mdhd", &mdhd), boxed(b"hdlr", &hdlr), minf].concat());
        let trak = boxed(b"trak", &[boxed(b"tkhd", &tkhd), mdia].concat());
        boxed(b"moov", &[boxed(b"mvhd", &mvhd), trak].concat())
    }
}

const MATRIX: [u8; 36] = [
    0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, //
    0, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, //
    0, 0, 0, 0, 0, 0, 0, 0, 0x40, 0, 0, 0,
];

fn boxed(kind: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let mut b = Vec::with_capacity(body.len() + 8);
    b.extend_from_slice(&((body.len() + 8) as u32).to_be_bytes());
    b.extend_from_slice(kind);
    b.extend_from_slice(body);
    b
}

fn full(version: u8, flags: u32) -> Vec<u8> {
    let mut v = flags.to_be_bytes().to_vec();
    v[0] = version;
    v
}

fn descriptor(tag: u8, body: &[u8]) -> Vec<u8> {
    let mut d = vec![tag];
    // 4-byte length form works for any size.
    let len = body.len() as u32;
    d.extend_from_slice(&[0x80 | ((len >> 21) & 0x7F) as u8, 0x80 | ((len >> 14) & 0x7F) as u8, 0x80 | ((len >> 7) & 0x7F) as u8, (len & 0x7F) as u8]);
    d.extend_from_slice(body);
    d
}

// ------------------------------------------------------------- ADTS ----

const ADTS_RATES: [u32; 13] = [96000, 88200, 64000, 48000, 44100, 32000, 24000, 22050, 16000, 12000, 11025, 8000, 7350];

/// Wraps an ADTS AAC stream into an .m4a file.
pub fn adts_to_m4a(src: &Path, dst: &Path) -> Result<(), String> {
    let mut f = BufReader::with_capacity(1 << 20, File::open(src).map_err(io)?);
    let mut writer: Option<Mp4Writer> = None;
    let mut frame = Vec::with_capacity(2048);
    loop {
        let mut h = [0u8; 7];
        if f.read_exact(&mut h).is_err() {
            break;
        }
        if h[0] != 0xFF || h[1] & 0xF6 != 0xF0 {
            // Lost sync (e.g. a cut-off last frame): skip a byte and retry.
            f.seek(SeekFrom::Current(-6)).map_err(io)?;
            continue;
        }
        let protection_absent = h[1] & 1 == 1;
        let profile = (h[2] >> 6) & 3;
        let sr_index = (h[2] >> 2) & 0xF;
        let channels = ((h[2] & 1) << 2) | (h[3] >> 6);
        let len = (((h[3] & 3) as usize) << 11) | ((h[4] as usize) << 3) | ((h[5] >> 5) as usize);
        let header_len = if protection_absent { 7 } else { 9 };
        if len < header_len || sr_index as usize >= ADTS_RATES.len() {
            f.seek(SeekFrom::Current(-6)).map_err(io)?;
            continue;
        }
        if !protection_absent {
            let mut crc = [0u8; 2];
            if f.read_exact(&mut crc).is_err() {
                break;
            }
        }
        frame.resize(len - header_len, 0);
        if f.read_exact(&mut frame).is_err() {
            break; // incomplete last frame
        }
        if writer.is_none() {
            let object_type = profile + 1;
            let asc = vec![(object_type << 3) | (sr_index >> 1), ((sr_index & 1) << 7) | (channels << 3)];
            writer = Some(Mp4Writer::create(dst, asc, ADTS_RATES[sr_index as usize], channels as u16)?);
        }
        writer.as_mut().unwrap().add_frame(&frame)?;
    }
    writer.ok_or_else(|| "The recording contains no audio.".to_string())?.finish()
}

// ------------------------------------------------------------- read ----

/// One AAC frame inside an .m4a file.
pub struct Frame {
    pub offset: u64,
    pub size: u32,
    /// Start time in seconds.
    pub time: f64,
}

pub struct M4aInfo {
    pub asc: Vec<u8>,
    pub sample_rate: u32,
    pub channels: u16,
    pub frames: Vec<Frame>,
}

struct Atom<'a> {
    kind: [u8; 4],
    body: &'a [u8],
}

fn atoms(mut data: &[u8]) -> Vec<Atom<'_>> {
    let mut out = Vec::new();
    while data.len() >= 8 {
        let mut size = u32::from_be_bytes(data[0..4].try_into().unwrap()) as u64;
        let kind: [u8; 4] = data[4..8].try_into().unwrap();
        let mut header = 8;
        if size == 1 && data.len() >= 16 {
            size = u64::from_be_bytes(data[8..16].try_into().unwrap());
            header = 16;
        } else if size == 0 {
            size = data.len() as u64;
        }
        if size < header as u64 || size > data.len() as u64 {
            break;
        }
        out.push(Atom { kind, body: &data[header..size as usize] });
        data = &data[size as usize..];
    }
    out
}

fn child<'a>(data: &'a [u8], kind: &[u8; 4]) -> Option<&'a [u8]> {
    atoms(data).into_iter().find(|a| &a.kind == kind).map(|a| a.body)
}

fn be32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn be64(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_be_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

/// Reads an MPEG-4 descriptor at `at`: (tag, body range).
fn read_descriptor(b: &[u8], mut at: usize) -> Option<(u8, usize, usize)> {
    let tag = *b.get(at)?;
    at += 1;
    let mut len = 0usize;
    for _ in 0..4 {
        let c = *b.get(at)?;
        at += 1;
        len = (len << 7) | (c & 0x7F) as usize;
        if c & 0x80 == 0 {
            break;
        }
    }
    Some((tag, at, (at + len).min(b.len())))
}

/// AudioSpecificConfig from an esds box body.
fn esds_asc(esds: &[u8]) -> Option<Vec<u8>> {
    let (tag, s, e) = read_descriptor(esds, 4)?;
    if tag != 3 {
        return None;
    }
    let es = &esds[s..e];
    let flags = *es.get(2)?;
    let mut at = 3;
    if flags & 0x80 != 0 {
        at += 2;
    }
    if flags & 0x40 != 0 {
        at += 1 + *es.get(at)? as usize;
    }
    if flags & 0x20 != 0 {
        at += 2;
    }
    let (tag, s, e) = read_descriptor(es, at)?;
    if tag != 4 {
        return None;
    }
    let dcd = &es[s..e];
    let (tag, s, e) = read_descriptor(dcd, 13)?;
    if tag != 5 {
        return None;
    }
    Some(dcd[s..e].to_vec())
}

/// Reads the audio track layout of an .m4a file.
pub fn read_m4a(src: &Path) -> Result<M4aInfo, String> {
    let bad = || "This M4A file has an unsupported layout.".to_string();
    let mut f = File::open(src).map_err(io)?;
    let len = f.metadata().map_err(io)?.len();

    // Find the moov box without loading the (large) mdat.
    let mut pos = 0u64;
    let moov = loop {
        if pos + 8 > len {
            return Err(bad());
        }
        f.seek(SeekFrom::Start(pos)).map_err(io)?;
        let mut h = [0u8; 16];
        f.read_exact(&mut h[..8]).map_err(io)?;
        let mut size = u32::from_be_bytes(h[0..4].try_into().unwrap()) as u64;
        let mut header = 8;
        if size == 1 {
            f.read_exact(&mut h[8..16]).map_err(io)?;
            size = u64::from_be_bytes(h[8..16].try_into().unwrap());
            header = 16;
        } else if size == 0 {
            size = len - pos;
        }
        if size < header {
            return Err(bad());
        }
        if &h[4..8] == b"moov" {
            let mut body = vec![0u8; (size - header) as usize];
            f.read_exact(&mut body).map_err(io)?;
            break body;
        }
        pos += size;
    };

    // First audio track.
    let trak = atoms(&moov)
        .into_iter()
        .filter(|a| &a.kind == b"trak")
        .find(|t| {
            child(t.body, b"mdia").and_then(|m| child(m, b"hdlr")).is_some_and(|h| h.get(8..12) == Some(b"soun"))
        })
        .ok_or_else(bad)?
        .body;
    let mdia = child(trak, b"mdia").ok_or_else(bad)?;
    let mdhd = child(mdia, b"mdhd").ok_or_else(bad)?;
    let timescale = if mdhd[0] == 1 { be32(mdhd, 20) } else { be32(mdhd, 12) }.ok_or_else(bad)?.max(1);
    let stbl = child(mdia, b"minf").and_then(|m| child(m, b"stbl")).ok_or_else(bad)?;

    let stsd = child(stbl, b"stsd").ok_or_else(bad)?;
    let entry = atoms(stsd.get(8..).ok_or_else(bad)?).into_iter().next().ok_or_else(bad)?;
    if &entry.kind != b"mp4a" {
        return Err("Only AAC audio in M4A files can be trimmed.".into());
    }
    let channels = u16::from_be_bytes(entry.body.get(16..18).ok_or_else(bad)?.try_into().unwrap());
    let esds = child(entry.body.get(28..).ok_or_else(bad)?, b"esds").ok_or_else(bad)?;
    let asc = esds_asc(esds).ok_or_else(bad)?;
    let sample_rate = {
        // Prefer the rate from the AudioSpecificConfig; fall back to the timescale.
        let idx = ((asc[0] & 7) << 1) | (asc.get(1).copied().unwrap_or(0) >> 7);
        ADTS_RATES.get(idx as usize).copied().unwrap_or(timescale)
    };

    // Sample sizes
    let stsz = child(stbl, b"stsz").ok_or_else(bad)?;
    let fixed = be32(stsz, 4).ok_or_else(bad)?;
    let count = be32(stsz, 8).ok_or_else(bad)? as usize;
    let sizes: Vec<u32> = if fixed != 0 {
        vec![fixed; count]
    } else {
        (0..count).map(|i| be32(stsz, 12 + i * 4)).collect::<Option<_>>().ok_or_else(bad)?
    };

    // Chunk offsets
    let chunk_offsets: Vec<u64> = if let Some(stco) = child(stbl, b"stco") {
        let n = be32(stco, 4).ok_or_else(bad)? as usize;
        (0..n).map(|i| be32(stco, 8 + i * 4).map(|v| v as u64)).collect::<Option<_>>().ok_or_else(bad)?
    } else {
        let co64 = child(stbl, b"co64").ok_or_else(bad)?;
        let n = be32(co64, 4).ok_or_else(bad)? as usize;
        (0..n).map(|i| be64(co64, 8 + i * 8)).collect::<Option<_>>().ok_or_else(bad)?
    };

    // Samples per chunk
    let stsc = child(stbl, b"stsc").ok_or_else(bad)?;
    let n = be32(stsc, 4).ok_or_else(bad)? as usize;
    let runs: Vec<(u32, u32)> = (0..n)
        .map(|i| Some((be32(stsc, 8 + i * 12)?, be32(stsc, 12 + i * 12)?)))
        .collect::<Option<_>>()
        .ok_or_else(bad)?;

    // Sample durations
    let stts = child(stbl, b"stts").ok_or_else(bad)?;
    let n = be32(stts, 4).ok_or_else(bad)? as usize;
    let mut durations = Vec::with_capacity(count);
    for i in 0..n {
        let c = be32(stts, 8 + i * 8).ok_or_else(bad)?;
        let d = be32(stts, 12 + i * 8).ok_or_else(bad)?;
        durations.extend(std::iter::repeat_n(d, c as usize));
    }

    let mut frames = Vec::with_capacity(count);
    let mut sample = 0usize;
    let mut t: u64 = 0;
    for (ci, &base) in chunk_offsets.iter().enumerate() {
        let chunk_no = ci as u32 + 1;
        let per_chunk = runs.iter().rev().find(|(first, _)| *first <= chunk_no).map(|r| r.1).unwrap_or(0);
        let mut off = base;
        for _ in 0..per_chunk {
            if sample >= count {
                break;
            }
            frames.push(Frame { offset: off, size: sizes[sample], time: t as f64 / timescale as f64 });
            off += sizes[sample] as u64;
            t += durations.get(sample).copied().unwrap_or(FRAME_SAMPLES) as u64;
            sample += 1;
        }
    }
    if frames.is_empty() {
        return Err(bad());
    }
    Ok(M4aInfo { asc, sample_rate, channels, frames })
}

/// Copies the frames between `start` and `end` (seconds) into a new .m4a.
pub fn trim_m4a(src: &Path, dst: &Path, start: f64, end: f64) -> Result<(), String> {
    let info = read_m4a(src)?;
    let frame_secs = FRAME_SAMPLES as f64 / info.sample_rate as f64;
    let mut f = BufReader::with_capacity(1 << 20, File::open(src).map_err(io)?);
    let mut w = Mp4Writer::create(dst, info.asc.clone(), info.sample_rate, info.channels)?;
    let mut buf = Vec::new();
    let mut pos = u64::MAX;
    for fr in &info.frames {
        let mid = fr.time + frame_secs / 2.0;
        if mid < start {
            continue;
        }
        if mid > end {
            break;
        }
        if pos != fr.offset {
            f.seek(SeekFrom::Start(fr.offset)).map_err(io)?;
        }
        buf.resize(fr.size as usize, 0);
        f.read_exact(&mut buf).map_err(io)?;
        pos = fr.offset + fr.size as u64;
        w.add_frame(&buf)?;
    }
    w.finish()
}
