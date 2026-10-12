//! Media Foundation, Windows' own media framework: a source reader decodes
//! a file (on disk, or in memory through a stream) to RGB32 pictures and
//! float sound, which are converted here to RGBA at the size asked for and
//! to stereo at the output's rate. Samples come in the file's order; the
//! player times them.

use std::cell::Cell;

use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::Win32::UI::Shell::SHCreateMemStream;
use windows::core::{GUID, HSTRING};

use super::decode::{Backend, Info, Out, Resampler, Source, to_rgba};

/// Joins this thread to COM's shared apartment, once (a thread already in
/// another stays there: Media Foundation's objects work from any).
fn com() {
    thread_local!(static DONE: Cell<bool> = const { Cell::new(false) });
    DONE.with(|d| {
        if !d.get() {
            d.set(true);
            let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        }
    });
}

/// Starts Media Foundation (missing on Windows N editions without the
/// Media Feature Pack).
pub fn load() -> Result<(), String> {
    com();
    unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL) }.map_err(|e| e.message())
}

/// A stream being read: its index in the file and whether it ended.
struct Stream {
    index: u32,
    done: bool,
}

struct Mf {
    reader: IMFSourceReader,
    info: Info,
    video: Option<Stream>,
    audio: Option<Stream>,
    /// Pictures as decoded: size and bytes per row (negative: bottom-up).
    frame: (usize, usize, isize),
    out_size: Option<(usize, usize)>,
    /// The sound's rate and channels as decoded, and to the output.
    sound: (u32, usize),
    resampler: Resampler,
}

// Used by one thread at a time; the source reader can be used from any.
unsafe impl Send for Mf {}

/// Opens `src` with Media Foundation.
pub fn open(src: &Source, want_video: bool) -> Result<Box<dyn Backend>, String> {
    com();
    unsafe { open_inner(src, want_video) }.map(|m| Box::new(m) as Box<dyn Backend>).map_err(|e| e.message())
}

unsafe fn open_inner(src: &Source, want_video: bool) -> windows::core::Result<Mf> {
    unsafe {
        let mut attrs = None;
        MFCreateAttributes(&mut attrs, 1)?;
        let attrs = attrs.ok_or_else(windows::core::Error::empty)?;
        // Decoding to RGB32 needs the reader's own color conversion.
        attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_VIDEO_PROCESSING, 1)?;
        let reader = match src {
            Source::Path(p) => MFCreateSourceReaderFromURL(&HSTRING::from(p.as_os_str()), &attrs)?,
            Source::Bytes(data) => {
                let stream = SHCreateMemStream(Some(data)).ok_or_else(windows::core::Error::empty)?;
                let bytes = MFCreateMFByteStreamOnStream(&stream)?;
                MFCreateSourceReaderFromByteStream(&bytes, &attrs)?
            }
        };
        // The first video and sound streams, and nothing else.
        reader.SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)?;
        let (mut video, mut audio) = (None, None);
        for i in 0..64u32 {
            let Ok(t) = reader.GetNativeMediaType(i, 0) else { break };
            let major = t.GetGUID(&MF_MT_MAJOR_TYPE)?;
            if major == MFMediaType_Video && want_video && video.is_none() {
                video = Some(Stream { index: i, done: false });
            } else if major == MFMediaType_Audio && audio.is_none() {
                audio = Some(Stream { index: i, done: false });
            }
        }
        let mut info = Info::default();
        let mut frame = (0, 0, 0);
        if let Some(s) = &video {
            let t = MFCreateMediaType()?;
            t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
            t.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32)?;
            if reader.SetCurrentMediaType(s.index, None, &t).is_ok() && reader.SetStreamSelection(s.index, true).is_ok() {
                let cur = reader.GetCurrentMediaType(s.index)?;
                let size = cur.GetUINT64(&MF_MT_FRAME_SIZE)?;
                let (w, h) = ((size >> 32) as usize, (size & 0xFFFF_FFFF) as usize);
                let stride = cur.GetUINT32(&MF_MT_DEFAULT_STRIDE).map_or(w as isize * 4, |s| s as i32 as isize);
                frame = (w, h, stride);
                info.size = Some((w.max(1), h.max(1)));
            } else {
                video = None;
            }
        }
        let mut sound = (48_000, 2);
        if let Some(s) = &audio {
            let t = MFCreateMediaType()?;
            t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
            t.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_Float)?;
            if reader.SetCurrentMediaType(s.index, None, &t).is_ok() && reader.SetStreamSelection(s.index, true).is_ok() {
                let cur = reader.GetCurrentMediaType(s.index)?;
                sound = (cur.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND).unwrap_or(48_000), cur.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS).unwrap_or(2).max(1) as usize);
                info.audio = true;
            } else {
                audio = None;
            }
        }
        if video.is_none() && audio.is_none() {
            return Err(windows::core::Error::new(MF_E_UNSUPPORTED_FORMAT, "nothing to play"));
        }
        // 100-nanosecond units.
        if let Ok(v) = reader.GetPresentationAttribute(MF_SOURCE_READER_MEDIASOURCE.0 as u32, &MF_PD_DURATION) {
            info.duration = u64::try_from(&v).map_or(0.0, |d| d as f64 / 1e7);
        }
        Ok(Mf { reader, info, video, audio, frame, out_size: None, sound, resampler: Resampler::new(sound.0, 48_000, sound.1) })
    }
}

impl Mf {
    /// A sample's bytes.
    unsafe fn bytes(sample: &IMFSample) -> windows::core::Result<Vec<u8>> {
        unsafe {
            let buf = sample.ConvertToContiguousBuffer()?;
            let (mut p, mut n) = (std::ptr::null_mut(), 0u32);
            buf.Lock(&mut p, None, Some(&mut n))?;
            let v = if p.is_null() { Vec::new() } else { std::slice::from_raw_parts(p, n as usize).to_vec() };
            buf.Unlock()?;
            Ok(v)
        }
    }

    /// The format of stream `index` changed: its new size or rate.
    unsafe fn reread(&mut self, index: u32) {
        unsafe {
            let Ok(cur) = self.reader.GetCurrentMediaType(index) else { return };
            if self.video.as_ref().is_some_and(|s| s.index == index) {
                if let Ok(size) = cur.GetUINT64(&MF_MT_FRAME_SIZE) {
                    let (w, h) = ((size >> 32) as usize, (size & 0xFFFF_FFFF) as usize);
                    let stride = cur.GetUINT32(&MF_MT_DEFAULT_STRIDE).map_or(w as isize * 4, |s| s as i32 as isize);
                    self.frame = (w, h, stride);
                }
            } else {
                let rate = cur.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND).unwrap_or(self.sound.0);
                let channels = cur.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS).map_or(self.sound.1, |c| c.max(1) as usize);
                self.sound = (rate, channels);
                self.resampler = Resampler::new(rate, self.resampler.to, channels);
            }
        }
    }

    fn picture(&self, t: f64, bytes: Vec<u8>) -> Option<Out> {
        let (w, h, stride) = self.frame;
        if w == 0 || h == 0 {
            return None;
        }
        let row = stride.unsigned_abs();
        // Bottom-up rows: top first.
        let bytes = if stride < 0 { bytes.chunks(row).take(h).rev().flatten().copied().collect() } else { bytes };
        let (dw, dh) = self.out_size.unwrap_or((w, h));
        Some(Out::Video { t, w: dw, h: dh, rgba: to_rgba(&bytes, w, h, row, true, (dw, dh)) })
    }
}

impl Backend for Mf {
    fn info(&self) -> Info {
        self.info
    }

    fn set_output(&mut self, size: Option<(usize, usize)>, rate: u32) {
        self.out_size = size;
        self.resampler = Resampler::new(self.sound.0, rate, self.sound.1);
    }

    fn seek(&mut self, t: f64) {
        com();
        let at = PROPVARIANT::from((t * 1e7) as i64);
        if let Err(e) = unsafe { self.reader.SetCurrentPosition(&GUID::zeroed(), &at) } {
            crate::diag::warning(&format!("Media Foundation: {}", e.message()));
        }
        for s in [&mut self.video, &mut self.audio].into_iter().flatten() {
            s.done = false;
        }
        self.resampler.reset();
    }

    fn next(&mut self) -> Option<Out> {
        com();
        loop {
            if [&self.video, &self.audio].into_iter().flatten().all(|s| s.done) {
                return None;
            }
            let (mut index, mut flags, mut ts) = (0u32, 0u32, 0i64);
            let mut sample = None;
            let r = unsafe { self.reader.ReadSample(MF_SOURCE_READER_ANY_STREAM.0 as u32, 0, Some(&mut index), Some(&mut flags), Some(&mut ts), Some(&mut sample)) };
            if let Err(e) = r {
                crate::diag::warning(&format!("Media Foundation: {}", e.message()));
                return None;
            }
            if flags & MF_SOURCE_READERF_ERROR.0 as u32 != 0 {
                return None;
            }
            if flags & MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED.0 as u32 != 0 {
                unsafe { self.reread(index) };
            }
            let is_video = self.video.as_ref().is_some_and(|s| s.index == index);
            if flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0 {
                for s in [&mut self.video, &mut self.audio].into_iter().flatten().filter(|s| s.index == index) {
                    s.done = true;
                }
            }
            let Some(sample) = sample else { continue };
            let Ok(bytes) = (unsafe { Self::bytes(&sample) }) else { continue };
            let t = ts as f64 / 1e7;
            if is_video {
                if let Some(o) = self.picture(t, bytes) {
                    return Some(o);
                }
            } else {
                let input: Vec<f32> = bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect();
                let samples = self.resampler.run(&input);
                if !samples.is_empty() {
                    return Some(Out::Audio { t, samples });
                }
            }
        }
    }
}
