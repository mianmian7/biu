//! Dual-backend decoder: Symphonia first, `ffmpeg` PCM pipe as fallback.
//!
//! Every backend produces interleaved **stereo** `f32` samples in `[-1.0, 1.0]`.

use std::collections::VecDeque;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow, bail};
use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::{Time, TimeBase, Timestamp};

use crate::source::AudioSource;

/// Give up on a stream after this many consecutive undecodable packets.
const MAX_CONSECUTIVE_DECODE_ERRORS: u32 = 64;
/// Frames read from the ffmpeg pipe per `decode_next` call.
const FFMPEG_CHUNK_FRAMES: usize = 2048;

/// Which backend is decoding the current track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecoderBackend {
    Symphonia,
    Ffmpeg,
}

/// A streaming stereo `f32` decoder.
pub trait Decoder: Send {
    /// Sample rate of the samples produced by [`decode_next`](Self::decode_next).
    fn sample_rate(&self) -> u32;
    /// Total duration if known.
    fn duration_secs(&self) -> Option<f64>;
    /// Current decode position (time of the next sample to be produced).
    fn position_secs(&self) -> f64;
    /// Decode the next block and **append** interleaved stereo samples to `out`.
    ///
    /// Returns `Ok(false)` at clean end of stream (nothing appended).
    fn decode_next(&mut self, out: &mut Vec<f32>) -> Result<bool>;
    /// Seek to `secs`; returns the actual position the next sample will be at.
    fn seek(&mut self, secs: f64) -> Result<f64>;
    fn backend(&self) -> DecoderBackend;
}

/// Options for [`open_decoder`].
#[derive(Debug, Clone, Default)]
pub struct DecoderOptions {
    /// Explicit ffmpeg binary; `None` means `ffmpeg` on `PATH`.
    pub ffmpeg_path: Option<PathBuf>,
    /// Output sample rate requested from ffmpeg (Symphonia keeps the native rate).
    pub ffmpeg_out_rate: u32,
    /// Disable the ffmpeg fallback entirely.
    pub disable_ffmpeg: bool,
    /// Once set, blocked reads of a still-downloading source fail promptly (track switch/stop).
    pub cancel: Option<Arc<AtomicBool>>,
}

/// Open the best decoder for `source`, positioned at `start_secs`.
///
/// Symphonia is tried first; if it cannot probe the container, create a decoder, or decode
/// the first audio block, ffmpeg is spawned instead.
pub fn open_decoder(
    source: &AudioSource,
    start_secs: f64,
    opts: &DecoderOptions,
) -> Result<Box<dyn Decoder>> {
    let start_secs = sanitize_secs(start_secs);
    let sym_err = match SymphoniaDecoder::open_cancellable(source.clone(), opts.cancel.clone()) {
        Ok(mut dec) => {
            if start_secs > 0.0 {
                dec.seek(start_secs)?;
            }
            return Ok(Box::new(dec));
        }
        Err(e) => e,
    };
    if opts
        .cancel
        .as_ref()
        .is_some_and(|c| c.load(Ordering::Acquire))
    {
        return Err(sym_err.context("cancelled"));
    }
    if opts.disable_ffmpeg {
        return Err(sym_err.context("symphonia failed and ffmpeg fallback is disabled"));
    }
    log::info!("[biu-audio] symphonia failed ({sym_err:#}); falling back to ffmpeg");
    let rate = if opts.ffmpeg_out_rate == 0 {
        48_000
    } else {
        opts.ffmpeg_out_rate
    };
    match FfmpegDecoder::spawn_cancellable(
        source.clone(),
        start_secs,
        rate,
        opts.ffmpeg_path.clone(),
        opts.cancel.clone(),
    ) {
        Ok(dec) => Ok(Box::new(dec)),
        Err(ff_err) => Err(anyhow!(
            "unable to decode audio: symphonia: {sym_err:#}; ffmpeg: {ff_err:#}"
        )),
    }
}

fn sanitize_secs(secs: f64) -> f64 {
    if secs.is_finite() { secs.max(0.0) } else { 0.0 }
}

/// Convert interleaved `channels`-channel samples to stereo, appending to `out`.
pub fn mix_to_stereo(input: &[f32], channels: usize, out: &mut Vec<f32>) {
    match channels {
        0 => {}
        1 => {
            out.reserve(input.len() * 2);
            for &s in input {
                let s = clamp_sample(s);
                out.push(s);
                out.push(s);
            }
        }
        2 => {
            out.reserve(input.len());
            out.extend(input.iter().map(|&s| clamp_sample(s)));
        }
        n => {
            // Assume SMPTE/WAVE order: FL FR [FC] [LFE] then surround pairs (L, R alternating).
            const ATT: f32 = std::f32::consts::FRAC_1_SQRT_2;
            let has_lfe = n >= 6;
            let mut wl = 1.0f32;
            let mut wr = 1.0f32;
            // Center contributes to both sides.
            wl += ATT;
            wr += ATT;
            let first_surround = if has_lfe { 4 } else { 3 };
            for c in first_surround..n {
                if (c - first_surround) % 2 == 0 {
                    wl += ATT;
                } else {
                    wr += ATT;
                }
            }
            out.reserve(input.len() / n * 2);
            for frame in input.chunks_exact(n) {
                let mut l = frame[0];
                let mut r = frame[1];
                let c = frame[2] * ATT;
                l += c;
                r += c;
                for (i, &s) in frame.iter().enumerate().skip(first_surround) {
                    if (i - first_surround) % 2 == 0 {
                        l += s * ATT;
                    } else {
                        r += s * ATT;
                    }
                }
                out.push(clamp_sample(l / wl * 1.4));
                out.push(clamp_sample(r / wr * 1.4));
            }
        }
    }
}

#[inline]
fn clamp_sample(s: f32) -> f32 {
    if s.is_nan() { 0.0 } else { s.clamp(-1.0, 1.0) }
}

// ---------------------------------------------------------------------------------------------
// Symphonia backend
// ---------------------------------------------------------------------------------------------

/// Symphonia-based decoder (MP4/fMP4 `.m4s`, M4A, FLAC, MP3, OGG Vorbis, WAV, ALAC, AAC, ...).
pub struct SymphoniaDecoder {
    source: AudioSource,
    cancel: Option<Arc<AtomicBool>>,
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    time_base: Option<TimeBase>,
    sample_rate: u32,
    duration_secs: Option<f64>,
    /// Frames emitted (or skipped) since time zero; position = pos_frames / sample_rate.
    pos_frames: u64,
    /// Frames still to discard after a seek for sample accuracy.
    skip_frames: u64,
    /// Primed audio decoded during `open` (ensures the stream is actually decodable).
    pending: VecDeque<f32>,
    scratch: Vec<f32>,
}

impl SymphoniaDecoder {
    /// Probe and open `source`, decoding the first block to prove it is playable.
    pub fn open(source: AudioSource) -> Result<Self> {
        Self::open_cancellable(source, None)
    }

    /// Like [`open`](Self::open); setting `cancel` interrupts blocked streaming reads.
    pub fn open_cancellable(source: AudioSource, cancel: Option<Arc<AtomicBool>>) -> Result<Self> {
        let mut dec = Self::open_unprimed(source, cancel)?;
        let mut first = Vec::new();
        if !dec.decode_raw(&mut first)? {
            bail!("stream contains no decodable audio");
        }
        dec.pending.extend(first);
        Ok(dec)
    }

    fn open_unprimed(source: AudioSource, cancel: Option<Arc<AtomicBool>>) -> Result<Self> {
        let ms = source
            .open_media_source_cancellable(cancel.as_slice())
            .context("open media source")?;
        let mss = MediaSourceStream::new(ms, Default::default());
        let mut hint = Hint::new();
        if let Some(ext) = source.hint_extension() {
            // DASH segments from Bilibili are fragmented MP4.
            let ext = match ext.as_str() {
                "m4s" | "m4a" | "mp4" | "m4b" => "mp4",
                other => other,
            };
            hint.with_extension(ext);
        }
        let format = symphonia::default::get_probe()
            .probe(
                &hint,
                mss,
                FormatOptions::default(),
                MetadataOptions::default(),
            )
            .map_err(|e| anyhow!("probe failed: {e}"))?;
        let track = format
            .default_track(TrackType::Audio)
            .ok_or_else(|| anyhow!("no audio track"))?;
        let params = track
            .codec_params
            .as_ref()
            .and_then(|p| p.audio())
            .ok_or_else(|| anyhow!("missing audio codec parameters"))?
            .clone();
        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(&params, &AudioDecoderOptions::default())
            .map_err(|e| anyhow!("unsupported codec: {e}"))?;
        let sample_rate = params.sample_rate.unwrap_or(0);
        let time_base = track.time_base;
        let duration_secs = match (time_base, track.duration) {
            (Some(tb), Some(d)) => tb.calc_duration(d).map(|t| t.as_secs_f64()),
            _ => None,
        }
        .or_else(|| {
            track
                .num_frames
                .filter(|_| sample_rate > 0)
                .map(|n| n as f64 / sample_rate as f64)
        })
        .filter(|d| d.is_finite() && *d > 0.0);
        let track_id = track.id;
        Ok(Self {
            source,
            cancel,
            format,
            decoder,
            track_id,
            time_base,
            sample_rate,
            duration_secs,
            pos_frames: 0,
            skip_frames: 0,
            pending: VecDeque::new(),
            scratch: Vec::new(),
        })
    }

    /// Re-create the decoder after a `ResetRequired` (e.g. chained OGG streams).
    fn reset_track(&mut self) -> Result<()> {
        let track = self
            .format
            .default_track(TrackType::Audio)
            .ok_or_else(|| anyhow!("no audio track after reset"))?;
        let params = track
            .codec_params
            .as_ref()
            .and_then(|p| p.audio())
            .ok_or_else(|| anyhow!("missing codec parameters after reset"))?
            .clone();
        self.decoder = symphonia::default::get_codecs()
            .make_audio_decoder(&params, &AudioDecoderOptions::default())
            .map_err(|e| anyhow!("unsupported codec after reset: {e}"))?;
        self.track_id = track.id;
        self.time_base = track.time_base;
        if let Some(r) = params.sample_rate {
            self.sample_rate = r;
        }
        Ok(())
    }

    /// Decode packets until at least one stereo frame is appended to `out` (after skip).
    fn decode_raw(&mut self, out: &mut Vec<f32>) -> Result<bool> {
        let mut consecutive_errors = 0u32;
        loop {
            let packet = match self.format.next_packet() {
                Ok(Some(p)) => p,
                Ok(None) => return Ok(false),
                Err(SymError::ResetRequired) => {
                    if self.reset_track().is_err() {
                        return Ok(false);
                    }
                    continue;
                }
                Err(SymError::IoError(e)) if e.kind() == io::ErrorKind::UnexpectedEof => {
                    return Ok(false);
                }
                Err(SymError::DecodeError(msg)) => {
                    // Corrupt container data: try to resync on the next packet.
                    consecutive_errors += 1;
                    if consecutive_errors >= MAX_CONSECUTIVE_DECODE_ERRORS {
                        bail!("too many demux errors: {msg}");
                    }
                    continue;
                }
                Err(e) => return Err(anyhow!("demux error: {e}")),
            };
            if packet.track_id != self.track_id {
                continue;
            }
            let buf = match self.decoder.decode(&packet) {
                Ok(buf) => buf,
                Err(SymError::DecodeError(msg)) => {
                    consecutive_errors += 1;
                    if consecutive_errors >= MAX_CONSECUTIVE_DECODE_ERRORS {
                        bail!("too many consecutive decode errors: {msg}");
                    }
                    log::debug!("[biu-audio] skipping corrupt packet: {msg}");
                    continue;
                }
                Err(SymError::IoError(e)) => {
                    consecutive_errors += 1;
                    if consecutive_errors >= MAX_CONSECUTIVE_DECODE_ERRORS {
                        bail!("too many consecutive decode io errors: {e}");
                    }
                    continue;
                }
                Err(e) => return Err(anyhow!("decode error: {e}")),
            };
            consecutive_errors = 0;
            let frames = buf.frames();
            if frames == 0 {
                continue;
            }
            let spec = buf.spec();
            let channels = spec.channels().count().max(1);
            if self.sample_rate == 0 || self.sample_rate != spec.rate() {
                self.sample_rate = spec.rate();
            }
            self.scratch.clear();
            buf.copy_to_vec_interleaved(&mut self.scratch);
            let skip = (self.skip_frames as usize).min(frames);
            self.skip_frames -= skip as u64;
            self.pos_frames += frames as u64;
            if skip == frames {
                continue;
            }
            let start = skip * channels;
            let end = (frames * channels).min(self.scratch.len());
            mix_to_stereo(&self.scratch[start..end], channels, out);
            return Ok(true);
        }
    }

    fn time_to_ts(&self, secs: f64) -> Option<Timestamp> {
        let time = Time::try_from_secs_f64(secs)?;
        match self.time_base {
            Some(tb) => tb.calc_timestamp(time),
            None if self.sample_rate > 0 => Some(Timestamp::new(
                (secs * self.sample_rate as f64).round() as i64,
            )),
            None => None,
        }
    }

    fn ts_to_secs(&self, ts: Timestamp) -> f64 {
        match self.time_base {
            Some(tb) => tb.calc_time_saturating(ts).as_secs_f64(),
            None if self.sample_rate > 0 => ts.get() as f64 / self.sample_rate as f64,
            None => 0.0,
        }
    }

    /// Fallback seek: reopen when going backwards, then decode-and-discard forward.
    fn seek_by_discard(&mut self, secs: f64) -> Result<f64> {
        if secs < self.position_secs() {
            let fresh = Self::open_unprimed(self.source.clone(), self.cancel.clone())?;
            *self = fresh;
        }
        self.pending.clear();
        let target = (secs * self.sample_rate.max(1) as f64).round() as u64;
        self.skip_frames = target.saturating_sub(self.pos_frames);
        Ok(secs)
    }
}

impl Decoder for SymphoniaDecoder {
    fn sample_rate(&self) -> u32 {
        self.sample_rate.max(1)
    }

    fn duration_secs(&self) -> Option<f64> {
        self.duration_secs
    }

    fn position_secs(&self) -> f64 {
        let pending_frames = (self.pending.len() / 2) as u64;
        let frames = (self.pos_frames + self.skip_frames).saturating_sub(pending_frames);
        frames as f64 / self.sample_rate.max(1) as f64
    }

    fn decode_next(&mut self, out: &mut Vec<f32>) -> Result<bool> {
        if !self.pending.is_empty() {
            out.extend(self.pending.drain(..));
            return Ok(true);
        }
        self.decode_raw(out)
    }

    fn seek(&mut self, secs: f64) -> Result<f64> {
        let mut secs = sanitize_secs(secs);
        if let Some(d) = self.duration_secs {
            secs = secs.min(d);
        }
        // A still-downloading stream cannot be byte-seeked safely (the target may not exist
        // yet); decode forward instead.
        let growing = self.source.is_growing();
        let Some(ts) = (!growing).then(|| self.time_to_ts(secs)).flatten() else {
            return self.seek_by_discard(secs);
        };
        let seeked = self.format.seek(
            SeekMode::Accurate,
            SeekTo::Timestamp {
                ts,
                track_id: self.track_id,
            },
        );
        match seeked {
            Ok(to) => {
                self.decoder.reset();
                self.pending.clear();
                let actual = self.ts_to_secs(to.actual_ts).max(0.0);
                let required = self.ts_to_secs(to.required_ts).max(actual);
                let rate = self.sample_rate.max(1) as f64;
                self.pos_frames = (actual * rate).round() as u64;
                self.skip_frames = ((required - actual) * rate).round() as u64;
                Ok(required)
            }
            Err(e) => {
                log::debug!("[biu-audio] container seek failed ({e}); seeking by decode");
                self.seek_by_discard(secs)
            }
        }
    }

    fn backend(&self) -> DecoderBackend {
        DecoderBackend::Symphonia
    }
}

// ---------------------------------------------------------------------------------------------
// ffmpeg backend
// ---------------------------------------------------------------------------------------------

/// Decodes by piping `ffmpeg -f f32le -ac 2` output.
pub struct FfmpegDecoder {
    source: AudioSource,
    ffmpeg: PathBuf,
    out_rate: u32,
    child: Option<ChildGuard>,
    stdout: Option<ChildStdout>,
    stderr_info: Arc<Mutex<StderrInfo>>,
    /// Engine-level cancel (track switch / stop) forwarded to the stdin feeder's source.
    cancel: Option<Arc<AtomicBool>>,
    /// Per-process feeder state; replaced on every (re)start.
    feeder: Option<FeederState>,
    start_secs: f64,
    frames_read: u64,
    leftover: Vec<u8>,
    byte_buf: Vec<u8>,
    pending: Vec<f32>,
}

/// Kills and reaps the ffmpeg process when dropped, so no early return can leak it.
struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct FeederState {
    /// Set when the process is torn down so a feeder blocked on a growing file exits promptly.
    stop: Arc<AtomicBool>,
    /// Read/open error hit while feeding ffmpeg's stdin (input truncated => not a clean EOF).
    error: Arc<Mutex<Option<String>>>,
}

impl Drop for FeederState {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}

#[derive(Default)]
struct StderrInfo {
    duration_secs: Option<f64>,
    tail: VecDeque<String>,
}

impl FfmpegDecoder {
    /// Spawn ffmpeg for `source` starting at `start_secs`, verifying that it produces audio.
    pub fn spawn(
        source: AudioSource,
        start_secs: f64,
        out_rate: u32,
        ffmpeg_path: Option<PathBuf>,
    ) -> Result<Self> {
        Self::spawn_cancellable(source, start_secs, out_rate, ffmpeg_path, None)
    }

    /// Like [`spawn`](Self::spawn); setting `cancel` interrupts a stdin feeder that is blocked
    /// on a still-downloading source.
    pub fn spawn_cancellable(
        source: AudioSource,
        start_secs: f64,
        out_rate: u32,
        ffmpeg_path: Option<PathBuf>,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<Self> {
        let ffmpeg = ffmpeg_path.unwrap_or_else(|| PathBuf::from(ffmpeg_binary_name()));
        let mut dec = Self {
            source,
            ffmpeg,
            out_rate: out_rate.max(8_000),
            child: None,
            stdout: None,
            stderr_info: Arc::new(Mutex::new(StderrInfo::default())),
            cancel,
            feeder: None,
            start_secs: 0.0,
            frames_read: 0,
            leftover: Vec::new(),
            byte_buf: vec![0u8; FFMPEG_CHUNK_FRAMES * 8],
            pending: Vec::new(),
        };
        dec.restart(sanitize_secs(start_secs))?;
        let mut first = Vec::new();
        if !dec.read_pcm(&mut first)? {
            bail!("ffmpeg produced no audio{}", dec.stderr_tail());
        }
        dec.pending = first;
        Ok(dec)
    }

    fn kill(&mut self) {
        self.stdout = None;
        self.feeder = None; // stops the old feeder
        self.child = None; // ChildGuard kills + reaps
    }

    fn restart(&mut self, start_secs: f64) -> Result<()> {
        self.kill();
        self.leftover.clear();
        self.pending.clear();
        self.frames_read = 0;
        self.start_secs = start_secs;

        let path_input = match &self.source {
            AudioSource::File(p) => Some(p.clone()),
            _ => None,
        };
        let mut cmd = Command::new(&self.ffmpeg);
        cmd.arg("-hide_banner").arg("-loglevel").arg("info");
        if path_input.is_some() {
            cmd.arg("-nostdin");
        }
        if start_secs > 0.0 {
            cmd.arg("-ss").arg(format!("{start_secs:.3}"));
        }
        cmd.arg("-i");
        match &path_input {
            Some(p) => cmd.arg(p),
            None => cmd.arg("pipe:0"),
        };
        cmd.args(["-vn", "-sn", "-dn", "-f", "f32le", "-acodec", "pcm_f32le"])
            .arg("-ac")
            .arg("2")
            .arg("-ar")
            .arg(self.out_rate.to_string())
            .arg("pipe:1")
            .stdin(if path_input.is_some() {
                Stdio::null()
            } else {
                Stdio::piped()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        // Guard immediately: any `?` below drops it, killing and reaping the process.
        let mut child = ChildGuard(
            cmd.spawn()
                .with_context(|| format!("spawn {}", self.ffmpeg.display()))?,
        );
        let feeder = FeederState {
            stop: Arc::new(AtomicBool::new(false)),
            error: Arc::new(Mutex::new(None)),
        };

        if let Some(mut stdin) = child.0.stdin.take() {
            let src = self.source.clone();
            let mut cancel = vec![feeder.stop.clone()];
            cancel.extend(self.cancel.clone());
            let error = feeder.error.clone();
            std::thread::Builder::new()
                .name("biu-ffmpeg-stdin".into())
                .spawn(move || {
                    let record = |msg: String| {
                        *error.lock().unwrap_or_else(|p| p.into_inner()) = Some(msg);
                    };
                    let mut ms = match src.open_media_source_cancellable(&cancel) {
                        Ok(ms) => ms,
                        Err(e) => return record(format!("open input: {e}")),
                    };
                    let mut buf = vec![0u8; 64 * 1024];
                    loop {
                        match ms.read(&mut buf) {
                            Ok(0) => break,
                            Ok(n) => {
                                if stdin.write_all(&buf[..n]).is_err() {
                                    break; // ffmpeg exited / was killed
                                }
                            }
                            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                            // Recorded before `stdin` drops, so the reader sees it at EOF.
                            Err(e) => return record(format!("read input: {e}")),
                        }
                    }
                })
                .context("spawn ffmpeg stdin feeder")?;
        }
        if let Some(stderr) = child.0.stderr.take() {
            let info = self.stderr_info.clone();
            std::thread::Builder::new()
                .name("biu-ffmpeg-stderr".into())
                .spawn(move || {
                    for line in BufReader::new(stderr).lines().map_while(|l| l.ok()) {
                        let mut info = info.lock().unwrap_or_else(|p| p.into_inner());
                        if info.duration_secs.is_none() {
                            info.duration_secs = parse_ffmpeg_duration(&line);
                        }
                        if info.tail.len() >= 8 {
                            info.tail.pop_front();
                        }
                        info.tail.push_back(line);
                    }
                })
                .context("spawn ffmpeg stderr reader")?;
        }
        self.stdout = child.0.stdout.take();
        self.child = Some(child);
        self.feeder = Some(feeder);
        Ok(())
    }

    fn stderr_tail(&self) -> String {
        let info = self.stderr_info.lock().unwrap_or_else(|p| p.into_inner());
        if info.tail.is_empty() {
            String::new()
        } else {
            format!(
                ": {}",
                info.tail.iter().cloned().collect::<Vec<_>>().join(" | ")
            )
        }
    }

    fn read_pcm(&mut self, out: &mut Vec<f32>) -> Result<bool> {
        let Some(stdout) = self.stdout.as_mut() else {
            return Ok(false);
        };
        loop {
            let n = match stdout.read(&mut self.byte_buf) {
                Ok(n) => n,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(anyhow!("ffmpeg pipe read failed: {e}")),
            };
            if n == 0 {
                self.stdout = None;
                let status = match self.child.as_mut() {
                    Some(c) => Some(c.0.wait().context("wait for ffmpeg")?),
                    None => None,
                };
                // Input truncated by a feeder failure is an error, not a clean end of stream.
                let feeder_err = self
                    .feeder
                    .as_ref()
                    .and_then(|f| f.error.lock().unwrap_or_else(|p| p.into_inner()).take());
                if let Some(msg) = feeder_err {
                    bail!("ffmpeg input failed: {msg}{}", self.stderr_tail());
                }
                if let Some(st) = status
                    && !st.success()
                {
                    bail!("ffmpeg exited with {st}{}", self.stderr_tail());
                }
                return Ok(false);
            }
            self.leftover.extend_from_slice(&self.byte_buf[..n]);
            let whole = self.leftover.len() / 8 * 8; // complete stereo frames only
            if whole == 0 {
                continue;
            }
            out.reserve(whole / 4);
            for b in self.leftover[..whole].as_chunks::<4>().0 {
                out.push(clamp_sample(f32::from_le_bytes([b[0], b[1], b[2], b[3]])));
            }
            self.leftover.drain(..whole);
            self.frames_read += (whole / 8) as u64;
            return Ok(true);
        }
    }
}

impl Drop for FfmpegDecoder {
    fn drop(&mut self) {
        self.kill();
    }
}

impl Decoder for FfmpegDecoder {
    fn sample_rate(&self) -> u32 {
        self.out_rate
    }

    fn duration_secs(&self) -> Option<f64> {
        self.stderr_info
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .duration_secs
    }

    fn position_secs(&self) -> f64 {
        let frames = self.frames_read - (self.pending.len() / 2) as u64;
        self.start_secs + frames as f64 / self.out_rate as f64
    }

    fn decode_next(&mut self, out: &mut Vec<f32>) -> Result<bool> {
        if !self.pending.is_empty() {
            out.append(&mut self.pending);
            return Ok(true);
        }
        self.read_pcm(out)
    }

    fn seek(&mut self, secs: f64) -> Result<f64> {
        let secs = sanitize_secs(secs);
        self.restart(secs)?;
        Ok(secs)
    }

    fn backend(&self) -> DecoderBackend {
        DecoderBackend::Ffmpeg
    }
}

fn ffmpeg_binary_name() -> &'static str {
    if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    }
}

/// Parse `Duration: 00:03:21.45,` from ffmpeg's stderr banner.
fn parse_ffmpeg_duration(line: &str) -> Option<f64> {
    let rest = line.trim_start().strip_prefix("Duration:")?.trim_start();
    let stamp = rest.split(',').next()?.trim();
    let mut parts = stamp.split(':');
    let h: f64 = parts.next()?.parse().ok()?;
    let m: f64 = parts.next()?.parse().ok()?;
    let s: f64 = parts.next()?.parse().ok()?;
    let total = h * 3600.0 + m * 60.0 + s;
    (total.is_finite() && total > 0.0).then_some(total)
}

/// Locate an ffmpeg binary: explicit path if it exists, else `ffmpeg` resolved on `PATH`.
pub fn resolve_ffmpeg(explicit: Option<&Path>) -> Option<PathBuf> {
    if let Some(p) = explicit {
        return p.is_file().then(|| p.to_path_buf());
    }
    let name = ffmpeg_binary_name();
    std::env::var_os("PATH").and_then(|paths| {
        std::env::split_paths(&paths)
            .map(|dir| dir.join(name))
            .find(|candidate| candidate.is_file())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::{sine_wav, stereo_ramp_wav};

    fn decode_all(dec: &mut dyn Decoder) -> Vec<f32> {
        let mut out = Vec::new();
        while dec.decode_next(&mut out).unwrap() {}
        out
    }

    #[test]
    fn decodes_wav_exact_sample_count_and_duration() {
        let wav = sine_wav(44_100, 2, 44_100 * 2, 440.0);
        let src = AudioSource::bytes(wav, Some("wav"));
        let mut dec = open_decoder(&src, 0.0, &DecoderOptions::default()).unwrap();
        assert_eq!(dec.backend(), DecoderBackend::Symphonia);
        assert_eq!(dec.sample_rate(), 44_100);
        let d = dec.duration_secs().unwrap();
        assert!((d - 2.0).abs() < 1e-3, "duration {d}");
        let pcm = decode_all(dec.as_mut());
        assert_eq!(pcm.len(), 44_100 * 2 * 2);
        assert!(pcm.iter().all(|s| (-1.0..=1.0).contains(s)));
        assert!((dec.position_secs() - 2.0).abs() < 1e-3);
    }

    #[test]
    fn seek_is_sample_accurate() {
        // Ramp encodes the frame index, so the first sample after seek reveals the position.
        let wav = stereo_ramp_wav(8_000, 8_000 * 4);
        let src = AudioSource::bytes(wav, Some("wav"));
        let mut dec = open_decoder(&src, 0.0, &DecoderOptions::default()).unwrap();
        for target in [1.5f64, 0.25, 3.0, 0.0] {
            let actual = dec.seek(target).unwrap();
            assert!((actual - target).abs() < 1e-3, "seek {target} -> {actual}");
            let mut out = Vec::new();
            assert!(dec.decode_next(&mut out).unwrap());
            let frame = crate::test_util::ramp_frame_index(out[0]);
            let expected = (target * 8_000.0).round() as i64;
            assert!(
                (frame - expected).abs() <= 1,
                "target {target}: got frame {frame}, expected {expected}"
            );
        }
        // Start offset via open_decoder.
        let mut dec = open_decoder(&src, 2.0, &DecoderOptions::default()).unwrap();
        let rest = decode_all(dec.as_mut());
        assert_eq!(rest.len(), 8_000 * 2 * 2);
    }

    #[test]
    fn growing_file_decodes_to_exact_eof() {
        use std::io::Write;
        use std::sync::atomic::Ordering;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grow.wav");
        let frames = 44_100usize;
        let wav = sine_wav(44_100, 2, frames, 440.0);
        std::fs::File::create(&path).unwrap();
        // total_bytes unknown: only `done` may end the stream.
        let src = AudioSource::streaming_file(&path, None);
        let AudioSource::StreamingFile { done, .. } = &src else {
            unreachable!()
        };
        let done = done.clone();
        let p2 = path.clone();
        let producer = std::thread::spawn(move || {
            let mut f = std::fs::OpenOptions::new().append(true).open(&p2).unwrap();
            for chunk in wav.chunks(3001) {
                f.write_all(chunk).unwrap();
                f.flush().unwrap();
                std::thread::sleep(std::time::Duration::from_millis(3));
            }
            // Stall before completion: the decoder must wait, not stop early.
            std::thread::sleep(std::time::Duration::from_millis(150));
            done.store(true, Ordering::Release);
        });
        let opts = DecoderOptions {
            disable_ffmpeg: true,
            ..Default::default()
        };
        let mut dec = open_decoder(&src, 0.0, &opts).unwrap();
        assert_eq!(dec.backend(), DecoderBackend::Symphonia);
        let pcm = decode_all(dec.as_mut());
        producer.join().unwrap();
        assert_eq!(
            pcm.len(),
            frames * 2,
            "no premature cutoff, no extra samples"
        );
    }

    #[test]
    fn mono_upmix_duplicates_channel() {
        let wav = sine_wav(22_050, 1, 22_050, 1000.0);
        let src = AudioSource::bytes(wav, Some("wav"));
        let mut dec = open_decoder(&src, 0.0, &DecoderOptions::default()).unwrap();
        let pcm = decode_all(dec.as_mut());
        assert_eq!(pcm.len(), 22_050 * 2);
        assert!(pcm.as_chunks::<2>().0.iter().all(|f| f[0] == f[1]));
    }

    #[test]
    fn downmix_51_is_bounded() {
        let frame = [1.0f32, -1.0, 1.0, 1.0, 1.0, -1.0];
        let mut out = Vec::new();
        mix_to_stereo(&frame.repeat(4), 6, &mut out);
        assert_eq!(out.len(), 8);
        assert!(out.iter().all(|s| (-1.0..=1.0).contains(s)));
        assert!(out[0] > 0.0);
    }

    #[test]
    fn garbage_fails_without_ffmpeg() {
        let src = AudioSource::bytes(vec![0x42u8; 4096], Some("bin"));
        let opts = DecoderOptions {
            disable_ffmpeg: true,
            ..Default::default()
        };
        assert!(open_decoder(&src, 0.0, &opts).is_err());
        // With a bogus ffmpeg path the error mentions both backends.
        let opts = DecoderOptions {
            ffmpeg_path: Some(PathBuf::from("/definitely/not/ffmpeg")),
            ..Default::default()
        };
        let err = open_decoder(&src, 0.0, &opts).err().unwrap().to_string();
        assert!(err.contains("symphonia") && err.contains("ffmpeg"), "{err}");
    }

    #[test]
    fn ffmpeg_fallback_decodes_when_available() {
        let Some(ff) = resolve_ffmpeg(None) else {
            eprintln!("ffmpeg not on PATH; skipping fallback integration test");
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tone.wav");
        std::fs::write(&path, sine_wav(44_100, 2, 44_100, 440.0)).unwrap();
        let mut dec = FfmpegDecoder::spawn(
            AudioSource::File(path.clone()),
            0.0,
            48_000,
            Some(ff.clone()),
        )
        .unwrap();
        let pcm = decode_all(&mut dec);
        let frames = pcm.len() / 2;
        assert!((frames as i64 - 48_000).abs() < 200, "frames {frames}");
        // Seek restarts the pipe at the new offset.
        assert_eq!(dec.seek(0.5).unwrap(), 0.5);
        let rest = decode_all(&mut dec);
        assert!(((rest.len() / 2) as i64 - 24_000).abs() < 200);
        // Bytes input goes through stdin.
        let mut dec = FfmpegDecoder::spawn(
            AudioSource::bytes(sine_wav(44_100, 2, 22_050, 440.0), Some("wav")),
            0.0,
            48_000,
            Some(ff),
        )
        .unwrap();
        let pcm = decode_all(&mut dec);
        assert!(((pcm.len() / 2) as i64 - 24_000).abs() < 200);
    }

    /// Write an executable shell script standing in for ffmpeg.
    #[cfg(unix)]
    fn fake_ffmpeg(dir: &Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("fake-ffmpeg");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    /// Decode until the first error or clean EOF.
    fn drain(dec: &mut dyn Decoder) -> Result<usize> {
        let mut frames = 0;
        let mut buf = Vec::new();
        loop {
            buf.clear();
            if !dec.decode_next(&mut buf)? {
                return Ok(frames);
            }
            frames += buf.len() / 2;
        }
    }

    #[cfg(unix)]
    #[test]
    fn ffmpeg_nonzero_exit_after_audio_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        // Emits 1000 silent stereo f32 frames, then fails mid-stream.
        let ff = fake_ffmpeg(
            dir.path(),
            "head -c 8000 /dev/zero; echo 'decode blew up' >&2; exit 3",
        );
        let input = dir.path().join("in.bin");
        std::fs::write(&input, [0u8; 16]).unwrap();
        let mut dec =
            FfmpegDecoder::spawn(AudioSource::File(input), 0.0, 48_000, Some(ff)).unwrap();
        let err = drain(&mut dec).unwrap_err().to_string();
        assert!(err.contains("exited"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn ffmpeg_feeder_read_error_is_an_error_not_eof() {
        let dir = tempfile::tempdir().unwrap();
        // "Decodes" by echoing stdin, so output ends exactly when the feeder stops.
        let ff = fake_ffmpeg(dir.path(), "cat");
        let path = dir.path().join("dl.part");
        std::fs::write(&path, vec![0u8; 64 * 1024]).unwrap();
        let src = AudioSource::streaming_file(&path, Some(10 << 20));
        let AudioSource::StreamingFile { error, .. } = &src else {
            unreachable!()
        };
        let error = error.clone();
        let mut dec = FfmpegDecoder::spawn(src, 0.0, 48_000, Some(ff)).unwrap();
        *error.lock().unwrap() = Some("http 500".into());
        let err = drain(&mut dec).unwrap_err().to_string();
        assert!(err.contains("http 500"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn ffmpeg_child_is_reaped_when_spawn_fails() {
        let dir = tempfile::tempdir().unwrap();
        let pid_file = dir.path().join("pid");
        // Writes its pid, produces no audio, and would otherwise linger for a minute.
        let ff = fake_ffmpeg(
            dir.path(),
            &format!("echo $$ > '{}'; exec sleep 60", pid_file.display()),
        );
        let input = dir.path().join("in.bin");
        std::fs::write(&input, [0u8; 16]).unwrap();
        let src = AudioSource::File(input);
        let handle = std::thread::spawn(move || {
            FfmpegDecoder::spawn(src, 0.0, 48_000, Some(ff)).map(|_| ())
        });
        // Wait for the child to start, then kill it from outside: spawn() sees EOF + failure.
        let start = std::time::Instant::now();
        let pid = loop {
            if let Ok(s) = std::fs::read_to_string(&pid_file)
                && let Ok(pid) = s.trim().parse::<u32>()
            {
                break pid;
            }
            assert!(start.elapsed() < std::time::Duration::from_secs(5));
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        let alive = |pid: u32| {
            Command::new("kill")
                .args(["-0", &pid.to_string()])
                .stderr(Stdio::null())
                .status()
                .is_ok_and(|s| s.success())
        };
        assert!(alive(pid));
        Command::new("kill").arg(pid.to_string()).status().unwrap();
        assert!(handle.join().unwrap().is_err());
        // Reaped (not a zombie / leaked child).
        assert!(!alive(pid), "ffmpeg child {pid} leaked");
    }

    #[test]
    fn parses_ffmpeg_duration() {
        assert_eq!(
            parse_ffmpeg_duration("  Duration: 00:03:21.50, start: 0.000000, bitrate: 1411 kb/s"),
            Some(201.5)
        );
        assert_eq!(parse_ffmpeg_duration("  Duration: N/A, bitrate: N/A"), None);
    }
}
