//! Windows' built-in AAC decoder for profiles the native decoder cannot render.
//! Load the optional media DLLs from the system directory only when opening such a file.

use super::{Downmix, Error, Info};
use libloading::os::windows::{LOAD_LIBRARY_SEARCH_SYSTEM32, Library};
use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::ptr;
use symphonia::core::audio::{Channels, Position};
use windows::Win32::Media::MediaFoundation::*;
use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};
use windows::core::{GUID, HRESULT, Interface, PCWSTR};

const AUDIO: u32 = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
type Startup = unsafe extern "system" fn(u32, u32) -> HRESULT;
type Shutdown = unsafe extern "system" fn() -> HRESULT;
type CreateType = unsafe extern "system" fn(*mut *mut c_void) -> HRESULT;
type CreateFile = unsafe extern "system" fn(
    MF_FILE_ACCESSMODE,
    MF_FILE_OPENMODE,
    MF_FILE_FLAGS,
    PCWSTR,
    *mut *mut c_void,
) -> HRESULT;
type CreateReader =
    unsafe extern "system" fn(*mut c_void, *mut c_void, *mut *mut c_void) -> HRESULT;

type Result<T> = std::result::Result<T, Error>;
fn failure(e: impl std::fmt::Display) -> Error {
    Error::Decode(e.to_string())
}

struct Runtime {
    shutdown: Shutdown,
    create_file: CreateFile,
    // The reader and media types must be released before unloading their implementation.
    _platform: Library,
    _readwrite: Library,
}

impl Drop for Runtime {
    fn drop(&mut self) {
        unsafe {
            let _ = (self.shutdown)();
            CoUninitialize();
        }
    }
}

impl Runtime {
    fn start(platform_name: &str) -> Result<(Self, CreateReader, CreateType)> {
        // ABI signatures match the named Media Foundation exports. Module handles stay
        // alive through the last call and Release. An N edition can fail any of these loads.
        unsafe {
            let platform = Library::load_with_flags(platform_name, LOAD_LIBRARY_SEARCH_SYSTEM32)
                .map_err(failure)?;
            let readwrite =
                Library::load_with_flags("mfreadwrite.dll", LOAD_LIBRARY_SEARCH_SYSTEM32)
                    .map_err(failure)?;
            let startup = *platform.get::<Startup>(b"MFStartup\0").map_err(failure)?;
            let shutdown = *platform.get::<Shutdown>(b"MFShutdown\0").map_err(failure)?;
            let create_type = *platform
                .get::<CreateType>(b"MFCreateMediaType\0")
                .map_err(failure)?;
            let create_file = *platform
                .get::<CreateFile>(b"MFCreateFile\0")
                .map_err(failure)?;
            let create_reader = *readwrite
                .get::<CreateReader>(b"MFCreateSourceReaderFromByteStream\0")
                .map_err(failure)?;
            CoInitializeEx(None, COINIT_MULTITHREADED)
                .ok()
                .map_err(failure)?;
            if let Err(e) = startup(MF_VERSION, MFSTARTUP_NOSOCKET).ok() {
                CoUninitialize();
                return Err(failure(e));
            }
            Ok((
                Self {
                    shutdown,
                    create_file,
                    _platform: platform,
                    _readwrite: readwrite,
                },
                create_reader,
                create_type,
            ))
        }
    }
}

// The source reader has no COM proxy on some Windows versions. Keep its construction, calls,
// Release and MFShutdown on one MTA thread, including when an offline player moves threads.
pub(crate) struct AacSource {
    pub(super) info: Info,
    channels: usize,
    raw_adts: bool,
    requests: std::sync::mpsc::Sender<Request>,
    replies: std::sync::mpsc::Receiver<(Vec<f32>, Result<usize>)>,
    thread: Option<std::thread::JoinHandle<()>>,
}

enum Request {
    Read(Vec<f32>, usize),
    Seek(u64, bool),
    Close,
}

impl Drop for AacSource {
    fn drop(&mut self) {
        let _ = self.requests.send(Request::Close);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl AacSource {
    pub(super) fn fallback(
        path: &Path,
        format: &mut dyn symphonia::core::formats::FormatReader,
        rate: u32,
        native: Option<&dyn symphonia::core::codecs::audio::AudioDecoder>,
        open: fn(&Path, Option<f64>) -> Result<Self>,
    ) -> Result<Option<Self>> {
        // An LC AudioSpecificConfig may explicitly signal that SBR is absent. A raw AAC
        // stream (or an abbreviated config) can carry SBR/PS in-band at a different clock.
        let core = native.map(|d| d.codec_params());
        if let Some([object, config, 0x56, 0xe5, flag, ..]) =
            core.and_then(|p| p.extra_data.as_deref())
            && object >> 3 == 2
            && ((object & 7) << 1 | config >> 7) != 15
            && config & 0x78 != 0
            && config & 7 == 0
            && flag & 0x80 == 0
        {
            return Ok(None);
        }
        // MF presentation duration can include an empty MP4 edit before the first sample.
        // Its reader starts the audio at time zero, so use the audio track's time span.
        let track_seconds = format
            .default_track(symphonia::core::formats::TrackType::Audio)
            .and_then(|t| {
                let tb = t.time_base?;
                Some(
                    t.duration?.get() as f64 * f64::from(tb.numer.get())
                        / f64::from(tb.denom.get()),
                )
            });
        let mut source = match open(path, track_seconds) {
            Ok(source) => source,
            Err(_) if native.is_some() => return Ok(None),
            Err(e) => return Err(e),
        };
        if core.is_some_and(|p| {
            p.sample_rate == Some(source.info.rate)
                && p.channels
                    .as_ref()
                    .is_some_and(|c| c.count() == source.channels)
        }) {
            return Ok(None);
        }
        if format.format_info().format == symphonia::core::formats::well_known::FORMAT_ID_ADTS {
            // Raw ADTS has no length header. Count compressed access units at the core rate.
            source.raw_adts = true;
            let mut ticks = 0u64;
            while let Some(packet) = super::next_packet(format).map_err(failure)? {
                ticks = ticks.saturating_add(packet.dur.get());
            }
            source.info.frames =
                Some((u128::from(ticks) * u128::from(source.info.rate) / u128::from(rate)) as u64);
        }
        Ok(Some(source))
    }

    pub(super) fn open(path: &Path, track_seconds: Option<f64>) -> Result<Self> {
        Self::open_with_platform(path, track_seconds, "mfplat.dll")
    }
    pub(super) fn open_with_platform(
        path: &Path,
        track_seconds: Option<f64>,
        platform_name: &'static str,
    ) -> Result<Self> {
        use std::sync::mpsc::channel;
        let (requests, inbox) = channel();
        let (tx, replies) = channel();
        let (opened, info) = channel();
        let path = path.to_owned();
        let thread = std::thread::Builder::new()
            .name("AAC decoder".into())
            .spawn(move || {
                let mut reader = match Reader::open(&path, track_seconds, platform_name) {
                    Ok(r) => r,
                    Err(e) => {
                        let _ = opened.send(Err(e));
                        return;
                    }
                };
                if opened.send(Ok((reader.info, reader.mix.channels))).is_err() {
                    return;
                }
                while let Ok(request) = inbox.recv() {
                    let result = match request {
                        Request::Read(mut buf, max) => {
                            let count = reader.read(&mut buf, max);
                            (buf, count)
                        }
                        Request::Seek(frame, raw) => {
                            (Vec::new(), reader.seek(frame, raw).map(|()| 0))
                        }
                        Request::Close => break,
                    };
                    if tx.send(result).is_err() {
                        break;
                    }
                }
            })
            .map_err(failure)?;
        let (info, channels) = info.recv().map_err(failure)??;
        Ok(Self {
            info,
            channels,
            raw_adts: false,
            requests,
            replies,
            thread: Some(thread),
        })
    }

    pub(super) fn read(&mut self, out: &mut Vec<f32>, max: usize) -> Result<usize> {
        // Hand the allocation back and forth. No PCM is copied across threads.
        self.requests
            .send(Request::Read(std::mem::take(out), max))
            .map_err(failure)?;
        let (buf, n) = self.replies.recv().map_err(failure)?;
        *out = buf;
        n
    }

    pub(super) fn seek(&mut self, frame: u64) -> Result<()> {
        self.requests
            .send(Request::Seek(frame, self.raw_adts))
            .map_err(failure)?;
        self.replies.recv().map_err(failure)?.1.map(|_| ())
    }
}

struct Reader {
    reader: IMFSourceReader,
    _runtime: Runtime,
    info: Info,
    mix: Downmix,
    delay: u64,
    limit: Option<u64>,
    // Cursor is in actual PCM frames. Source timestamps re-establish it after a seek.
    cursor: Option<i64>,
    skip_to: u64,
    pending: Vec<f32>,
    at: usize,
    done: bool,
}

impl Reader {
    fn open(path: &Path, track_seconds: Option<f64>, platform_name: &str) -> Result<Self> {
        let (runtime, create_reader, create_type) = Runtime::start(platform_name)?;
        let local = std::path::absolute(path).map_err(failure)?;
        let wide: Vec<u16> = local.as_os_str().encode_wide().chain(Some(0)).collect();
        unsafe {
            let mut raw = ptr::null_mut();
            (runtime.create_file)(
                MF_ACCESSMODE_READ,
                MF_OPENMODE_FAIL_IF_NOT_EXIST,
                MF_FILEFLAGS_NONE,
                PCWSTR(wide.as_ptr()),
                &mut raw,
            )
            .ok()
            .map_err(failure)?;
            let stream = IMFByteStream::from_raw(raw);
            let mut raw = ptr::null_mut();
            create_reader(stream.as_raw(), ptr::null_mut(), &mut raw)
                .ok()
                .map_err(failure)?;
            let reader = IMFSourceReader::from_raw(raw);
            reader
                .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
                .map_err(failure)?;
            reader.SetStreamSelection(AUDIO, true).map_err(failure)?;
            let mut raw = ptr::null_mut();
            create_type(&mut raw).ok().map_err(failure)?;
            let mt = IMFMediaType::from_raw(raw);
            mt.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
                .map_err(failure)?;
            mt.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_Float)
                .map_err(failure)?;
            reader
                .SetCurrentMediaType(AUDIO, None, &mt)
                .map_err(failure)?;
            let (rate, mix) = output_format(&reader)?;
            let duration = reader
                .GetPresentationAttribute(MF_SOURCE_READER_MEDIASOURCE.0 as u32, &MF_PD_DURATION)
                .ok()
                .and_then(|v| u64::try_from(&v).ok());
            let frames = duration.map(|v| time_to_frame(v as i64, rate).max(0) as u64);
            let gapless = crate::mp4::gapless(path, rate);
            let delay = gapless.map_or(0, |g| g.delay);
            let limit = gapless.and_then(|g| g.length);
            drop(mt);
            drop(stream);
            let mut source = Self {
                reader,
                _runtime: runtime,
                info: Info {
                    rate,
                    frames: limit
                        .or_else(|| {
                            track_seconds.map(|s| {
                                ((s * f64::from(rate)).round() as u64).saturating_sub(delay)
                            })
                        })
                        .or(frames),
                },
                mix,
                delay,
                limit,

                cursor: Some(-(delay as i64)),
                skip_to: 0,
                pending: Vec::new(),
                at: 0,
                done: false,
            };
            source.next()?;
            if source.done && source.pending.is_empty() {
                return Err(failure("empty AAC stream"));
            }
            Ok(source)
        }
    }

    fn read(&mut self, out: &mut Vec<f32>, max: usize) -> Result<usize> {
        let mut got = 0;
        while got < max {
            if self.at == self.pending.len() {
                if self.done {
                    break;
                }
                self.next()?;
                continue;
            }
            let n = ((self.pending.len() - self.at) / 2).min(max - got);
            out.extend_from_slice(&self.pending[self.at..self.at + n * 2]);
            self.at += n * 2;
            got += n;
        }
        Ok(got)
    }

    fn seek(&mut self, frame: u64, raw_adts: bool) -> Result<()> {
        let preroll = u64::from(self.info.rate) / 4;
        let time = (u128::from(frame.saturating_add(self.delay).saturating_sub(preroll))
            * 10_000_000
            / u128::from(self.info.rate))
        .min(i64::MAX as u128) as i64;
        let time = if raw_adts { 0 } else { time };
        unsafe {
            self.reader
                .SetCurrentPosition(&GUID::zeroed(), &PROPVARIANT::from(time))
        }
        .map_err(failure)?;
        self.cursor = None;
        self.skip_to = frame;
        self.pending.clear();
        self.at = 0;
        self.done = false;
        Ok(())
    }

    fn next(&mut self) -> Result<()> {
        self.pending.clear();
        self.at = 0;
        let reader = &self.reader;
        // A synchronous reader can return a stream tick or a format change without samples.
        for _ in 0..256 {
            let (mut flags, mut time, mut sample) = (0, 0, None);
            unsafe {
                reader.ReadSample(
                    AUDIO,
                    0,
                    None,
                    Some(&mut flags),
                    Some(&mut time),
                    Some(&mut sample),
                )
            }
            .map_err(failure)?;
            if flags & MF_SOURCE_READERF_ERROR.0 as u32 != 0 {
                return Err(failure("AAC reader failed"));
            }
            if flags & MF_SOURCE_READERF_CURRENTMEDIATYPECHANGED.0 as u32 != 0 {
                let (rate, mix) = output_format(reader)?;
                if rate != self.info.rate {
                    return Err(failure("AAC sample rate changed"));
                }
                self.mix = mix;
            }
            self.done = flags & MF_SOURCE_READERF_ENDOFSTREAM.0 as u32 != 0;
            if let Some(sample) = sample {
                let buffer = unsafe { sample.ConvertToContiguousBuffer() }.map_err(failure)?;
                let (mut data, mut len) = (ptr::null_mut(), 0u32);
                unsafe { buffer.Lock(&mut data, None, Some(&mut len)) }.map_err(failure)?;
                // MF supplies interleaved IEEE float PCM. Check the buffer before forming a slice.
                let count = len as usize / 4;
                let valid = !data.is_null()
                    && data.cast::<f32>().is_aligned()
                    && (len as usize).is_multiple_of(4 * self.mix.channels);
                if valid {
                    let values = unsafe { std::slice::from_raw_parts(data.cast::<f32>(), count) };
                    let frames = count / self.mix.channels;
                    let first = *self.cursor.get_or_insert_with(|| {
                        time_to_frame(time, self.info.rate) - self.delay as i64
                    });
                    self.cursor = Some(first.saturating_add(frames as i64));
                    let from = (i128::from(self.skip_to) - i128::from(first))
                        .clamp(0, frames as i128) as usize;
                    let to = self.limit.map_or(frames, |end| {
                        (i128::from(end) - i128::from(first)).clamp(0, frames as i128) as usize
                    });
                    self.done |= to < frames;
                    self.mix.apply(values, from.min(to), to, &mut self.pending);
                }
                unsafe { buffer.Unlock() }.map_err(failure)?;
                if !valid && len != 0 {
                    return Err(failure("invalid AAC output buffer"));
                }
                if count > 0 {
                    return Ok(());
                }
            }
            if self.done {
                return Ok(());
            }
        }
        Err(failure("AAC reader returned no audio"))
    }
}

fn time_to_frame(time: i64, rate: u32) -> i64 {
    ((i128::from(time) * i128::from(rate) + 5_000_000).div_euclid(10_000_000)) as i64
}

fn output_format(reader: &IMFSourceReader) -> Result<(u32, Downmix)> {
    unsafe {
        let mt = reader.GetCurrentMediaType(AUDIO).map_err(failure)?;
        let rate = mt
            .GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND)
            .map_err(failure)?;
        let channels = mt.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS).map_err(failure)? as usize;
        if !(1..=64).contains(&channels)
            || rate == 0
            || mt.GetGUID(&MF_MT_SUBTYPE).map_err(failure)? != MFAudioFormat_Float
        {
            return Err(failure("unsupported AAC output format"));
        }
        let mask = mt.GetUINT32(&MF_MT_AUDIO_CHANNEL_MASK).unwrap_or(0);
        let mask = if mask == 0 && channels == 6 {
            0x3f
        } else {
            mask
        };
        let layout = (mask.count_ones() as usize == channels)
            .then(|| Channels::Positioned(Position::from_bits_truncate(u64::from(mask))));
        Ok((rate, Downmix::new(layout.as_ref(), channels)))
    }
}
