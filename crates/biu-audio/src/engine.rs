//! `AudioEngine`: non-blocking playback handle driving a `cpal` output stream.
//!
//! Threads:
//! * **control thread** (one per engine) — owns the `cpal::Stream` (which is `!Send` on some
//!   platforms), the state machine, idle-stream teardown and the spectrum analyzer.
//! * **decode worker** (one per started track) — opens the decoder (which may block on a
//!   still-downloading file or an ffmpeg spawn), decodes, resamples and pushes into the ring.
//!   A stalled worker therefore never blocks pause/stop/play on the control thread.
//! * **audio callback** — lock-free: pops from [`SampleRing`], applies a click-free gain ramp.
//!
//! When no output device exists (headless CI) the engine runs a *null sink*: the control thread
//! drains the ring in real time, so the clock, `TrackEnded` events and the queue keep working.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{Result, anyhow, bail};
use tokio::sync::broadcast;

use crate::decoder::{DecoderOptions, open_decoder};
use crate::resampler::{Resampler, clamp_rate};
use crate::ring::{PushResult, SampleRing, TapRing};
use crate::source::AudioSource;
use crate::spectrum::{FFT_SIZE, SPECTRUM_BARS, SpectrumAnalyzer};

/// ~0.5 s of stereo at 48 kHz: 48 000 f32 samples = 192 KB.
const RING_CAPACITY_SAMPLES: usize = 48_000;
const TAP_CAPACITY: usize = 4096;
const DEFAULT_OUT_RATE: u32 = 48_000;
const SPECTRUM_INTERVAL: Duration = Duration::from_millis(33);
/// Gain ramp time constant (click-free pause/volume changes).
const GAIN_RAMP_SECS: f32 = 0.004;
const RENDER_CHUNK_FRAMES: usize = 4096;

/// High-level playback state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
#[repr(u8)]
pub enum PlaybackState {
    /// Nothing loaded yet.
    #[default]
    Idle = 0,
    /// Opening / probing the source.
    Loading = 1,
    Playing = 2,
    Paused = 3,
    Stopped = 4,
    /// Track finished naturally.
    Ended = 5,
    /// Decoding failed; see `PlaybackSnapshot::error`.
    Error = 6,
}

impl PlaybackState {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Loading,
            2 => Self::Playing,
            3 => Self::Paused,
            4 => Self::Stopped,
            5 => Self::Ended,
            6 => Self::Error,
            _ => Self::Idle,
        }
    }

    pub fn is_playing(self) -> bool {
        self == Self::Playing
    }
}

/// Point-in-time view of the engine.
#[derive(Debug, Clone, PartialEq)]
pub struct PlaybackSnapshot {
    pub state: PlaybackState,
    pub position_secs: f64,
    pub duration_secs: f64,
    pub volume: f32,
    pub muted: bool,
    pub rate: f32,
    pub error: Option<String>,
    pub track_generation: u64,
    /// `false` when running on the headless null sink (no audio hardware).
    pub has_audio_device: bool,
}

/// Engine notifications.
#[derive(Debug, Clone, PartialEq)]
pub enum AudioEvent {
    /// The track started by `play()` with this generation finished naturally.
    TrackEnded {
        generation: u64,
    },
    StateChanged(PlaybackState),
    Error {
        generation: u64,
        message: String,
    },
}

/// Where audio goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OutputMode {
    /// Default system output device, falling back to the null sink if unavailable.
    #[default]
    Auto,
    /// Never touch audio hardware; real-time null sink clock (tests, headless).
    Null,
}

#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub output: OutputMode,
    /// Sample rate of the null sink.
    pub null_sample_rate: u32,
    /// Tear down the output stream after being paused/ended this long.
    pub idle_stream_timeout: Duration,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            output: OutputMode::Auto,
            null_sample_rate: DEFAULT_OUT_RATE,
            idle_stream_timeout: Duration::from_secs(5),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------------------------

struct Shared {
    state: AtomicU8,
    generation: AtomicU64,
    position_bits: AtomicU64,
    duration_bits: AtomicU64,
    volume_bits: AtomicU32,
    muted: AtomicBool,
    rate_bits: AtomicU32,
    error: Mutex<Option<String>>,
    waveform_enabled: AtomicBool,
    spectrum: Mutex<[f32; SPECTRUM_BARS]>,
    /// Callback should consume and play the ring (false = paused/loading/stopped).
    output_active: AtomicBool,
    out_rate: AtomicU32,
    has_device: AtomicBool,
    stream_alive: AtomicBool,
    ffmpeg_path: Mutex<Option<PathBuf>>,
    events: broadcast::Sender<AudioEvent>,
    ring: SampleRing,
    tap: TapRing,
}

fn load_f32(a: &AtomicU32) -> f32 {
    f32::from_bits(a.load(Ordering::Relaxed))
}
fn store_f32(a: &AtomicU32, v: f32) {
    a.store(v.to_bits(), Ordering::Relaxed);
}
fn load_f64(a: &AtomicU64) -> f64 {
    f64::from_bits(a.load(Ordering::Relaxed))
}
fn store_f64(a: &AtomicU64, v: f64) {
    a.store(v.to_bits(), Ordering::Relaxed);
}
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

impl Shared {
    fn state(&self) -> PlaybackState {
        PlaybackState::from_u8(self.state.load(Ordering::Acquire))
    }

    fn set_state(&self, s: PlaybackState) {
        let old = self.state.swap(s as u8, Ordering::AcqRel);
        if old != s as u8 {
            let _ = self.events.send(AudioEvent::StateChanged(s));
        }
    }

    fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }
}

// ---------------------------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------------------------

enum Cmd {
    Play {
        source: AudioSource,
        start_secs: f64,
        generation: u64,
    },
    Pause,
    Resume,
    Toggle,
    Stop,
    Seek(f64),
    RateChanged,
    StreamError(String),
    Worker(WorkerMsg),
    /// Test hook: change the null sink rate used the next time output is (re)opened, to
    /// simulate the default device switching sample rate while the stream was torn down.
    #[cfg(test)]
    SetNullRate(u32),
    Shutdown,
}

enum WorkerMsg {
    Loaded {
        worker_id: u64,
        duration_secs: Option<f64>,
    },
    Eof {
        worker_id: u64,
        generation: u64,
        epoch: u64,
    },
    Failed {
        worker_id: u64,
        generation: u64,
        message: String,
    },
}

enum WorkerCmd {
    Seek { secs: f64, epoch: u64 },
}

// ---------------------------------------------------------------------------------------------
// Public handle
// ---------------------------------------------------------------------------------------------

/// Cheap-to-clone, `Send + Sync` handle. The engine shuts down when the last clone drops.
#[derive(Clone)]
pub struct AudioEngine {
    inner: Arc<EngineInner>,
}

struct EngineInner {
    tx: Sender<Cmd>,
    shared: Arc<Shared>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Drop for EngineInner {
    fn drop(&mut self) {
        let _ = self.tx.send(Cmd::Shutdown);
        if let Some(h) = lock(&self.thread).take() {
            let _ = h.join();
        }
    }
}

impl Default for AudioEngine {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioEngine {
    /// Create an engine using the default output device (null sink if none). Never panics.
    pub fn new() -> Self {
        Self::with_config(EngineConfig::default())
    }

    pub fn with_config(config: EngineConfig) -> Self {
        let (events, _) = broadcast::channel(64);
        let shared = Arc::new(Shared {
            state: AtomicU8::new(PlaybackState::Idle as u8),
            generation: AtomicU64::new(0),
            position_bits: AtomicU64::new(0f64.to_bits()),
            duration_bits: AtomicU64::new(0f64.to_bits()),
            volume_bits: AtomicU32::new(1f32.to_bits()),
            muted: AtomicBool::new(false),
            rate_bits: AtomicU32::new(1f32.to_bits()),
            error: Mutex::new(None),
            waveform_enabled: AtomicBool::new(false),
            spectrum: Mutex::new([0.0; SPECTRUM_BARS]),
            output_active: AtomicBool::new(false),
            out_rate: AtomicU32::new(config.null_sample_rate.max(8_000)),
            has_device: AtomicBool::new(false),
            stream_alive: AtomicBool::new(false),
            ffmpeg_path: Mutex::new(None),
            events,
            ring: SampleRing::new(RING_CAPACITY_SAMPLES),
            tap: TapRing::new(TAP_CAPACITY),
        });
        let (tx, rx) = mpsc::channel();
        let control_tx = tx.clone();
        let control_shared = shared.clone();
        let thread = std::thread::Builder::new()
            .name("biu-audio-control".into())
            .spawn(move || Control::new(control_shared, rx, control_tx, config).run())
            .map_err(|e| log::error!("[biu-audio] failed to spawn control thread: {e}"))
            .ok();
        Self {
            inner: Arc::new(EngineInner {
                tx,
                shared,
                thread: Mutex::new(thread),
            }),
        }
    }

    fn send(&self, cmd: Cmd) {
        let _ = self.inner.tx.send(cmd);
    }

    fn shared(&self) -> &Shared {
        &self.inner.shared
    }

    /// Start playing `source` from `start_secs`. Bumps `track_generation`.
    pub fn play(&self, source: AudioSource, start_secs: f64) -> u64 {
        let s = self.shared();
        let generation = s.generation.fetch_add(1, Ordering::AcqRel) + 1;
        *lock(&s.error) = None;
        store_f64(&s.position_bits, sanitize(start_secs));
        store_f64(&s.duration_bits, 0.0);
        s.set_state(PlaybackState::Loading);
        self.send(Cmd::Play {
            source,
            start_secs,
            generation,
        });
        generation
    }

    pub fn pause(&self) {
        self.send(Cmd::Pause);
    }

    pub fn resume(&self) {
        self.send(Cmd::Resume);
    }

    pub fn toggle_play(&self) {
        self.send(Cmd::Toggle);
    }

    pub fn stop(&self) {
        self.send(Cmd::Stop);
    }

    pub fn seek(&self, secs: f64) {
        self.send(Cmd::Seek(sanitize(secs)));
    }

    /// Linear volume `0.0..=1.0` (same curve as HTML `audio.volume`).
    pub fn set_volume(&self, volume: f32) {
        let v = if volume.is_finite() {
            volume.clamp(0.0, 1.0)
        } else {
            1.0
        };
        store_f32(&self.shared().volume_bits, v);
    }

    pub fn set_muted(&self, muted: bool) {
        self.shared().muted.store(muted, Ordering::Relaxed);
    }

    /// Playback speed `0.5..=2.0` (pitch follows speed).
    pub fn set_rate(&self, rate: f32) {
        let rate = clamp_rate(rate);
        let old = self
            .shared()
            .rate_bits
            .swap(rate.to_bits(), Ordering::Relaxed);
        if old != rate.to_bits() {
            self.send(Cmd::RateChanged);
        }
    }

    /// Explicit ffmpeg binary for the fallback decoder (`None` = `ffmpeg` on `PATH`).
    pub fn set_ffmpeg_path(&self, path: Option<PathBuf>) {
        *lock(&self.shared().ffmpeg_path) = path;
    }

    /// Enable spectrum analysis (full-screen player open). Disabled = zero analysis cost.
    pub fn set_waveform_enabled(&self, enabled: bool) {
        self.shared()
            .waveform_enabled
            .store(enabled, Ordering::Relaxed);
    }

    pub fn snapshot(&self) -> PlaybackSnapshot {
        let s = self.shared();
        PlaybackSnapshot {
            state: s.state(),
            position_secs: load_f64(&s.position_bits),
            duration_secs: load_f64(&s.duration_bits),
            volume: load_f32(&s.volume_bits),
            muted: s.muted.load(Ordering::Relaxed),
            rate: load_f32(&s.rate_bits),
            error: lock(&s.error).clone(),
            track_generation: s.generation(),
            has_audio_device: s.has_device.load(Ordering::Relaxed),
        }
    }

    pub fn spectrum(&self) -> [f32; SPECTRUM_BARS] {
        *lock(&self.shared().spectrum)
    }

    pub fn subscribe_events(&self) -> broadcast::Receiver<AudioEvent> {
        self.shared().events.subscribe()
    }

    /// Whether an output stream (device or null sink) is currently open.
    pub fn is_output_open(&self) -> bool {
        self.shared().stream_alive.load(Ordering::Relaxed)
    }
}

fn sanitize(secs: f64) -> f64 {
    if secs.is_finite() { secs.max(0.0) } else { 0.0 }
}

// ---------------------------------------------------------------------------------------------
// Renderer (shared by the cpal callback and the null sink)
// ---------------------------------------------------------------------------------------------

struct Renderer {
    shared: Arc<Shared>,
    gain: f32,
    ramp_coef: f32,
    scratch: Vec<f32>,
}

impl Renderer {
    fn new(shared: Arc<Shared>, rate: u32) -> Self {
        let ramp_coef = 1.0 - (-1.0 / (GAIN_RAMP_SECS * rate.max(1) as f32)).exp();
        Self {
            shared,
            gain: 0.0,
            ramp_coef,
            scratch: vec![0.0; RENDER_CHUNK_FRAMES * 2],
        }
    }

    /// Render up to `RENDER_CHUNK_FRAMES` stereo frames into `self.scratch[..frames*2]`.
    fn render_chunk(&mut self, frames: usize) -> &[f32] {
        let frames = frames.min(RENDER_CHUNK_FRAMES);
        let s = &*self.shared;
        let active = s.output_active.load(Ordering::Relaxed);
        let target = if active && !s.muted.load(Ordering::Relaxed) {
            load_f32(&s.volume_bits)
        } else {
            0.0
        };
        let out = &mut self.scratch[..frames * 2];
        let max_pop_frames = if active {
            frames
        } else if self.gain > 1e-4 {
            // Drain at most a short (~11 ms @ 48 kHz) click-free fade-out tail on pause
            // instead of consuming an entire 4096-frame (85 ms) render chunk.
            frames.min(512)
        } else {
            self.gain = 0.0;
            0
        };
        let filled = if max_pop_frames > 0 {
            s.ring.pop(&mut out[..max_pop_frames * 2])
        } else {
            0
        };
        out[filled..].fill(0.0);
        let tap = s.waveform_enabled.load(Ordering::Relaxed);
        for f in out[..filled].as_chunks_mut::<2>().0 {
            if tap {
                s.tap.push((f[0] + f[1]) * 0.5);
            }
            self.gain += (target - self.gain) * self.ramp_coef;
            f[0] *= self.gain;
            f[1] *= self.gain;
        }
        if !active {
            // Ensure the fade-out tail completes in a single pause window.
            self.gain = 0.0;
        } else if filled < out.len() {
            // Underrun while active: preserve current gain.
        }
        &self.scratch[..frames * 2]
    }

    fn render_device<T>(&mut self, data: &mut [T], channels: usize)
    where
        T: cpal::Sample + cpal::FromSample<f32>,
    {
        let channels = channels.max(1);
        for block in data.chunks_mut(RENDER_CHUNK_FRAMES * channels) {
            let frames = block.len() / channels;
            let rendered = self.render_chunk(frames);
            for (frame, lr) in block.chunks_mut(channels).zip(rendered.as_chunks::<2>().0) {
                if channels == 1 {
                    frame[0] = T::from_sample((lr[0] + lr[1]) * 0.5);
                } else {
                    frame[0] = T::from_sample(lr[0]);
                    frame[1] = T::from_sample(lr[1]);
                    for x in &mut frame[2..] {
                        *x = T::EQUILIBRIUM;
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Output backends
// ---------------------------------------------------------------------------------------------

enum Output {
    Device { _stream: cpal::Stream },
    Null(NullSink),
}

struct NullSink {
    renderer: Renderer,
    rate: u32,
    last: Instant,
    carry: f64,
}

impl NullSink {
    fn new(shared: Arc<Shared>, rate: u32) -> Self {
        Self {
            renderer: Renderer::new(shared, rate),
            rate,
            last: Instant::now(),
            carry: 0.0,
        }
    }

    /// Consume audio at real-time speed since the last pump.
    fn pump(&mut self) {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last).as_secs_f64().min(0.25);
        self.last = now;
        let exact = elapsed * self.rate as f64 + self.carry;
        let mut frames = exact.floor() as usize;
        self.carry = exact - frames as f64;
        while frames > 0 {
            let n = frames.min(RENDER_CHUNK_FRAMES);
            self.renderer.render_chunk(n);
            frames -= n;
        }
    }
}

fn build_device_output(shared: &Arc<Shared>, tx: &Sender<Cmd>) -> Result<(Output, u32)> {
    use cpal::SampleFormat as F;
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    let host = cpal::default_host();
    let device = host
        .default_output_device()
        .ok_or_else(|| anyhow!("no default output device"))?;
    let supported = device
        .default_output_config()
        .map_err(|e| anyhow!("default output config: {e}"))?;
    let format = supported.sample_format();
    let config: cpal::StreamConfig = supported.into();
    let rate = config.sample_rate;
    let channels = config.channels as usize;

    fn build<T>(
        device: &cpal::Device,
        config: cpal::StreamConfig,
        channels: usize,
        shared: Arc<Shared>,
        tx: Sender<Cmd>,
    ) -> Result<cpal::Stream>
    where
        T: cpal::SizedSample + cpal::FromSample<f32>,
    {
        let mut renderer = Renderer::new(shared, config.sample_rate);
        let err_tx = Mutex::new(tx);
        device
            .build_output_stream(
                config,
                move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
                    renderer.render_device(data, channels)
                },
                move |err| {
                    log::warn!("[biu-audio] output stream error: {err}");
                    let _ = lock(&err_tx).send(Cmd::StreamError(err.to_string()));
                },
                None,
            )
            .map_err(|e| anyhow!("build output stream: {e}"))
    }

    let (s, t) = (shared.clone(), tx.clone());
    let stream = match format {
        F::F32 => build::<f32>(&device, config, channels, s, t)?,
        F::F64 => build::<f64>(&device, config, channels, s, t)?,
        F::I8 => build::<i8>(&device, config, channels, s, t)?,
        F::I16 => build::<i16>(&device, config, channels, s, t)?,
        F::I24 => build::<cpal::I24>(&device, config, channels, s, t)?,
        F::I32 => build::<i32>(&device, config, channels, s, t)?,
        F::I64 => build::<i64>(&device, config, channels, s, t)?,
        F::U8 => build::<u8>(&device, config, channels, s, t)?,
        F::U16 => build::<u16>(&device, config, channels, s, t)?,
        F::U32 => build::<u32>(&device, config, channels, s, t)?,
        F::U64 => build::<u64>(&device, config, channels, s, t)?,
        other => bail!("unsupported output sample format {other}"),
    };
    stream.play().map_err(|e| anyhow!("start stream: {e}"))?;
    Ok((Output::Device { _stream: stream }, rate))
}

// ---------------------------------------------------------------------------------------------
// Control thread
// ---------------------------------------------------------------------------------------------

struct WorkerHandle {
    id: u64,
    cancel: Arc<AtomicBool>,
    tx: Sender<WorkerCmd>,
}

impl Drop for WorkerHandle {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }
}

struct Control {
    shared: Arc<Shared>,
    rx: Receiver<Cmd>,
    tx: Sender<Cmd>,
    config: EngineConfig,
    output: Option<Output>,
    worker: Option<WorkerHandle>,
    next_worker_id: u64,
    /// Generation of the last `Cmd::Play` actually processed. Events are tagged with this, never
    /// with `Shared::generation`, which `play()` bumps before its command reaches this thread.
    active_gen: u64,
    /// Generation whose decoder reported EOF (valid while `decoder_done`).
    done_gen: u64,
    source: Option<AudioSource>,
    want_playing: bool,
    decoder_done: bool,
    idle_since: Option<Instant>,
    analyzer: SpectrumAnalyzer,
    tap_buf: Vec<f32>,
    last_tap_written: usize,
    last_spectrum: Instant,
    spectrum_dirty: bool,
}

impl Control {
    fn new(shared: Arc<Shared>, rx: Receiver<Cmd>, tx: Sender<Cmd>, config: EngineConfig) -> Self {
        let rate = shared.out_rate.load(Ordering::Relaxed);
        Self {
            shared,
            rx,
            tx,
            config,
            output: None,
            worker: None,
            next_worker_id: 0,
            active_gen: 0,
            done_gen: 0,
            source: None,
            want_playing: false,
            decoder_done: false,
            idle_since: None,
            analyzer: SpectrumAnalyzer::new(rate),
            tap_buf: vec![0.0; FFT_SIZE],
            last_tap_written: 0,
            last_spectrum: Instant::now(),
            spectrum_dirty: false,
        }
    }

    fn run(mut self) {
        loop {
            let timeout = self.tick_interval();
            match self.rx.recv_timeout(timeout) {
                Ok(Cmd::Shutdown) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(cmd) => {
                    self.handle(cmd);
                    // Coalesce bursts (e.g. rapid play() calls) before ticking.
                    let mut shutdown = false;
                    while let Ok(cmd) = self.rx.try_recv() {
                        if matches!(cmd, Cmd::Shutdown) {
                            shutdown = true;
                            break;
                        }
                        self.handle(cmd);
                    }
                    if shutdown {
                        break;
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
            }
            self.tick();
        }
        self.worker = None;
        self.drop_output();
    }

    fn tick_interval(&self) -> Duration {
        let state = self.shared.state();
        if state == PlaybackState::Playing {
            Duration::from_millis(10)
        } else if self.spectrum_dirty && self.shared.waveform_enabled.load(Ordering::Relaxed) {
            SPECTRUM_INTERVAL
        } else if self.output.is_some() {
            Duration::from_millis(250)
        } else {
            Duration::from_secs(1)
        }
    }

    fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Play {
                source,
                start_secs,
                generation,
            } => {
                if generation != self.shared.generation() {
                    return; // superseded by a newer play() still in the queue
                }
                self.active_gen = generation;
                self.source = Some(source);
                self.start_track(sanitize(start_secs), true);
            }
            Cmd::Pause => self.pause(),
            Cmd::Resume => self.resume(),
            Cmd::Toggle => match self.shared.state() {
                PlaybackState::Playing => self.pause(),
                PlaybackState::Loading => self.want_playing = !self.want_playing,
                _ => self.resume(),
            },
            Cmd::Stop => self.stop(),
            Cmd::Seek(secs) => self.seek(secs),
            Cmd::RateChanged => self.apply_rate_now(),
            Cmd::StreamError(msg) => {
                log::warn!("[biu-audio] recreating output after stream error: {msg}");
                self.drop_output();
                if self.shared.state() == PlaybackState::Playing {
                    self.ensure_output();
                }
            }
            Cmd::Worker(msg) => self.handle_worker(msg),
            #[cfg(test)]
            Cmd::SetNullRate(rate) => self.config.null_sample_rate = rate,
            Cmd::Shutdown => {}
        }
    }

    fn handle_worker(&mut self, msg: WorkerMsg) {
        let current = self.worker.as_ref().map(|w| w.id);
        match msg {
            WorkerMsg::Loaded {
                worker_id,
                duration_secs,
            } if Some(worker_id) == current => {
                store_f64(&self.shared.duration_bits, duration_secs.unwrap_or(0.0));
                if self.shared.state() != PlaybackState::Loading {
                    return;
                }
                if self.want_playing {
                    self.start_output_playing();
                } else {
                    self.shared.set_state(PlaybackState::Paused);
                    self.idle_since = Some(Instant::now());
                }
            }
            WorkerMsg::Eof {
                worker_id,
                generation,
                epoch,
            } if Some(worker_id) == current && epoch == self.shared.ring.epoch() => {
                self.decoder_done = true;
                self.done_gen = generation;
            }
            WorkerMsg::Failed {
                worker_id,
                generation,
                message,
            } if Some(worker_id) == current => {
                log::warn!("[biu-audio] playback error: {message}");
                self.worker = None;
                self.shared.output_active.store(false, Ordering::Release);
                *lock(&self.shared.error) = Some(message.clone());
                self.shared.set_state(PlaybackState::Error);
                self.idle_since = Some(Instant::now());
                let _ = self.shared.events.send(AudioEvent::Error {
                    generation,
                    message,
                });
            }
            _ => {} // stale worker
        }
    }

    /// (Re)start decoding the current source at `start_secs`.
    fn start_track(&mut self, start_secs: f64, play: bool) {
        let Some(source) = self.source.clone() else {
            return;
        };
        self.worker = None; // cancels the previous worker
        self.shared.output_active.store(false, Ordering::Release);
        let epoch = self.shared.ring.reset(start_secs);
        store_f64(&self.shared.position_bits, start_secs);
        *lock(&self.shared.error) = None;
        self.decoder_done = false;
        self.want_playing = play;
        self.shared.set_state(PlaybackState::Loading);
        if play {
            // Open the device while the decoder probes, to cut start latency.
            self.ensure_output();
        }

        self.next_worker_id += 1;
        let id = self.next_worker_id;
        let cancel = Arc::new(AtomicBool::new(false));
        let (wtx, wrx) = mpsc::channel();
        let generation = self.active_gen;
        let ctx = WorkerCtx {
            id,
            generation,
            source,
            start_secs,
            epoch,
            cancel: cancel.clone(),
            rx: wrx,
            events: self.tx.clone(),
            shared: self.shared.clone(),
        };
        match std::thread::Builder::new()
            .name("biu-audio-decode".into())
            .spawn(move || worker_main(ctx))
        {
            Ok(_) => {
                self.worker = Some(WorkerHandle {
                    id,
                    cancel,
                    tx: wtx,
                })
            }
            Err(e) => {
                let message = format!("failed to spawn decode thread: {e}");
                *lock(&self.shared.error) = Some(message.clone());
                self.shared.set_state(PlaybackState::Error);
                let _ = self.shared.events.send(AudioEvent::Error {
                    generation,
                    message,
                });
            }
        }
    }

    fn start_output_playing(&mut self) {
        self.want_playing = true;
        self.idle_since = None;
        self.ensure_output();
        self.shared.output_active.store(true, Ordering::Release);
        self.shared.set_state(PlaybackState::Playing);
    }

    fn pause(&mut self) {
        match self.shared.state() {
            PlaybackState::Playing => {
                if let Some(Output::Null(sink)) = &mut self.output {
                    sink.pump();
                }
                self.want_playing = false;
                self.shared.output_active.store(false, Ordering::Release);
                if let Some(pos) = self.shared.ring.position() {
                    store_f64(&self.shared.position_bits, pos.max(0.0));
                }
                self.shared.set_state(PlaybackState::Paused);
                self.idle_since = Some(Instant::now());
            }
            PlaybackState::Loading => self.want_playing = false,
            _ => {}
        }
    }

    fn resume(&mut self) {
        match self.shared.state() {
            PlaybackState::Paused => self.start_output_playing(),
            PlaybackState::Loading => self.want_playing = true,
            PlaybackState::Ended | PlaybackState::Stopped => self.start_track(0.0, true),
            PlaybackState::Error => {
                let pos = load_f64(&self.shared.position_bits);
                self.start_track(pos, true);
            }
            PlaybackState::Playing | PlaybackState::Idle => {}
        }
    }

    fn stop(&mut self) {
        if self.shared.state() == PlaybackState::Idle {
            return;
        }
        self.worker = None;
        self.want_playing = false;
        self.decoder_done = false;
        self.shared.output_active.store(false, Ordering::Release);
        self.shared.ring.reset(0.0);
        store_f64(&self.shared.position_bits, 0.0);
        self.drop_output();
        self.shared.set_state(PlaybackState::Stopped);
    }

    fn seek(&mut self, secs: f64) {
        let duration = load_f64(&self.shared.duration_bits);
        let secs = if duration > 0.0 {
            secs.min(duration)
        } else {
            secs
        };
        match (&self.worker, self.shared.state()) {
            (Some(w), PlaybackState::Loading | PlaybackState::Playing | PlaybackState::Paused) => {
                let epoch = self.shared.ring.reset(secs);
                let _ = w.tx.send(WorkerCmd::Seek { secs, epoch });
                self.decoder_done = false;
                store_f64(&self.shared.position_bits, secs);
            }
            (_, PlaybackState::Ended | PlaybackState::Stopped | PlaybackState::Error) => {
                // Re-open at the target, paused (like an HTML media element after `ended`).
                self.start_track(secs, false);
            }
            _ => {}
        }
    }

    /// Make a rate change audible immediately instead of after the buffered ~0.5 s: flush the
    /// ring and re-decode from the current playing position at the new rate. Growing files are
    /// skipped (a backwards seek would re-decode from the start); they pick up the new rate as
    /// the buffer drains.
    fn apply_rate_now(&mut self) {
        let state = self.shared.state();
        if self.worker.is_none()
            || !matches!(state, PlaybackState::Playing | PlaybackState::Paused)
            || self.decoder_done
            || self.source.as_ref().is_none_or(|s| s.is_growing())
        {
            return;
        }
        if let Some(pos) = self.shared.ring.position() {
            self.seek(pos);
        }
    }

    fn ensure_output(&mut self) {
        if self.output.is_some() {
            return;
        }
        let device = match self.config.output {
            OutputMode::Null => None,
            OutputMode::Auto => match build_device_output(&self.shared, &self.tx) {
                Ok(out) => Some(out),
                Err(e) => {
                    log::warn!("[biu-audio] no audio output ({e:#}); using null sink");
                    None
                }
            },
        };
        let (output, rate, has_device) = match device {
            Some((out, rate)) => (out, rate, true),
            None => {
                let rate = self.config.null_sample_rate.max(8_000);
                (
                    Output::Null(NullSink::new(self.shared.clone(), rate)),
                    rate,
                    false,
                )
            }
        };
        let old_rate = self.shared.out_rate.swap(rate, Ordering::AcqRel);
        self.shared.has_device.store(has_device, Ordering::Relaxed);
        self.shared.stream_alive.store(true, Ordering::Relaxed);
        self.analyzer.set_sample_rate(rate);
        self.output = Some(output);
        if old_rate != rate {
            self.resync_after_rate_change(old_rate, rate);
        }
    }

    /// The (re)opened output runs at a different sample rate (e.g. the default device changed
    /// while the stream was released during an idle pause). Everything buffered was resampled
    /// for the old rate and would play at the wrong pitch/speed, and would skew the clock, so
    /// flush it and re-decode from the current position; the worker picks up the new
    /// `out_rate` for every chunk it resamples.
    fn resync_after_rate_change(&mut self, old_rate: u32, new_rate: u32) {
        let state = self.shared.state();
        if self.worker.is_none()
            || !matches!(
                state,
                PlaybackState::Loading | PlaybackState::Playing | PlaybackState::Paused
            )
        {
            return;
        }
        log::info!("[biu-audio] output rate changed {old_rate} -> {new_rate} Hz; resyncing");
        if let Some(pos) = self.shared.ring.position() {
            self.seek(pos);
        }
    }

    fn drop_output(&mut self) {
        if self.output.take().is_some() {
            self.shared.stream_alive.store(false, Ordering::Relaxed);
        }
    }

    fn end_track(&mut self) {
        let generation = self.done_gen;
        self.worker = None;
        self.want_playing = false;
        self.decoder_done = false;
        self.shared.output_active.store(false, Ordering::Release);
        let duration = load_f64(&self.shared.duration_bits);
        let pos = load_f64(&self.shared.position_bits);
        if duration <= 0.0 {
            store_f64(&self.shared.duration_bits, pos);
        } else {
            store_f64(&self.shared.position_bits, duration);
        }
        self.shared.set_state(PlaybackState::Ended);
        self.idle_since = Some(Instant::now());
        let _ = self
            .shared
            .events
            .send(AudioEvent::TrackEnded { generation });
    }

    fn tick(&mut self) {
        if let Some(Output::Null(sink)) = &mut self.output {
            sink.pump();
        }
        let state = self.shared.state();
        if matches!(state, PlaybackState::Playing | PlaybackState::Paused)
            && let Some(pos) = self.shared.ring.position()
        {
            let duration = load_f64(&self.shared.duration_bits);
            let pos = if duration > 0.0 {
                pos.min(duration)
            } else {
                pos
            };
            store_f64(&self.shared.position_bits, pos.max(0.0));
        }
        if state == PlaybackState::Playing && self.decoder_done && self.shared.ring.len() == 0 {
            self.end_track();
        }
        self.tick_spectrum(state == PlaybackState::Playing);
        // Power: release CoreAudio/WASAPI/ALSA when idle.
        if self.output.is_some()
            && self.shared.state() != PlaybackState::Playing
            && self.shared.state() != PlaybackState::Loading
            && self
                .idle_since
                .is_some_and(|t| t.elapsed() >= self.config.idle_stream_timeout)
        {
            self.drop_output();
        }
    }

    fn tick_spectrum(&mut self, playing: bool) {
        if !self.shared.waveform_enabled.load(Ordering::Relaxed) {
            if self.spectrum_dirty {
                self.analyzer.reset();
                *lock(&self.shared.spectrum) = [0.0; SPECTRUM_BARS];
                self.spectrum_dirty = false;
            }
            return;
        }
        if self.last_spectrum.elapsed() < SPECTRUM_INTERVAL {
            return;
        }
        self.last_spectrum = Instant::now();
        let written = self.shared.tap.written();
        if playing && written != self.last_tap_written {
            self.last_tap_written = written;
            self.shared.tap.latest(&mut self.tap_buf);
            self.analyzer.push_mono(&self.tap_buf);
            self.analyzer.analyze();
            self.spectrum_dirty = true;
        } else if self.spectrum_dirty {
            self.spectrum_dirty = self.analyzer.decay();
        } else {
            return;
        }
        *lock(&self.shared.spectrum) = self.analyzer.bars();
    }
}

// ---------------------------------------------------------------------------------------------
// Decode worker
// ---------------------------------------------------------------------------------------------

struct WorkerCtx {
    id: u64,
    generation: u64,
    source: AudioSource,
    start_secs: f64,
    epoch: u64,
    cancel: Arc<AtomicBool>,
    rx: Receiver<WorkerCmd>,
    events: Sender<Cmd>,
    shared: Arc<Shared>,
}

impl WorkerCtx {
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
    }

    fn send(&self, msg: WorkerMsg) {
        if !self.cancelled() {
            let _ = self.events.send(Cmd::Worker(msg));
        }
    }

    fn fail(&self, message: String) {
        self.send(WorkerMsg::Failed {
            worker_id: self.id,
            generation: self.generation,
            message,
        });
    }
}

fn worker_main(ctx: WorkerCtx) {
    let opts = DecoderOptions {
        ffmpeg_path: lock(&ctx.shared.ffmpeg_path).clone(),
        ffmpeg_out_rate: ctx.shared.out_rate.load(Ordering::Acquire),
        disable_ffmpeg: false,
        cancel: Some(ctx.cancel.clone()),
    };
    let mut dec = match open_decoder(&ctx.source, ctx.start_secs, &opts) {
        Ok(d) => d,
        Err(e) => {
            if ctx.cancelled() {
                return;
            }
            return ctx.fail(format!("{e:#}"));
        }
    };
    if ctx.cancelled() {
        return;
    }
    ctx.send(WorkerMsg::Loaded {
        worker_id: ctx.id,
        duration_secs: dec.duration_secs(),
    });

    let mut epoch = ctx.epoch;
    let mut resampler = Resampler::new(
        dec.sample_rate(),
        ctx.shared.out_rate.load(Ordering::Acquire),
    );
    let mut src_secs = dec.position_secs();
    let mut decoded: Vec<f32> = Vec::with_capacity(8192);
    let mut pending: Vec<f32> = Vec::with_capacity(16384);
    let mut off = 0usize;
    let (mut seg_start, mut seg_end) = (src_secs, src_secs);
    let mut eof = false;
    let mut eof_sent = false;
    let mut carry: Option<WorkerCmd> = None;

    loop {
        if ctx.cancelled() {
            return;
        }
        // Apply the latest queued seek.
        let mut seek = carry.take();
        loop {
            match ctx.rx.try_recv() {
                Ok(cmd) => seek = Some(cmd),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => return,
            }
        }
        if let Some(WorkerCmd::Seek { secs, epoch: e }) = seek {
            match dec.seek(secs) {
                Ok(actual) => src_secs = actual,
                Err(_) if ctx.cancelled() => return,
                Err(err) => return ctx.fail(format!("seek failed: {err:#}")),
            }
            epoch = e;
            resampler.reset();
            pending.clear();
            off = 0;
            eof = false;
            eof_sent = false;
        }

        if off >= pending.len() && !eof {
            decoded.clear();
            match dec.decode_next(&mut decoded) {
                Ok(true) => {}
                Ok(false) => eof = true,
                Err(err) => {
                    if ctx.cancelled() {
                        return;
                    }
                    return ctx.fail(format!("{err:#}"));
                }
            }
            resampler.set_rate(load_f32(&ctx.shared.rate_bits));
            resampler.set_output_rate(ctx.shared.out_rate.load(Ordering::Relaxed));
            resampler.set_source_rate(dec.sample_rate());
            pending.clear();
            off = 0;
            if eof {
                resampler.flush(&mut pending);
            } else {
                resampler.process(&decoded, &mut pending);
            }
            seg_start = src_secs;
            src_secs += (decoded.len() / 2) as f64 / dec.sample_rate() as f64;
            seg_end = src_secs;
        }

        if off < pending.len() {
            let total = pending.len() as f64;
            let start = seg_start + (seg_end - seg_start) * (off as f64 / total);
            let wait = match ctx.shared.ring.push(epoch, &pending[off..], start, seg_end) {
                PushResult::Pushed(0) => Some(Duration::from_millis(15)),
                PushResult::Pushed(n) => {
                    off += n;
                    None
                }
                // A seek/new track is in flight; wait for its command.
                PushResult::Stale => Some(Duration::from_millis(5)),
            };
            if let Some(d) = wait {
                match ctx.rx.recv_timeout(d) {
                    Ok(cmd) => carry = Some(cmd),
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return,
                }
            }
            continue;
        }

        if eof {
            if !eof_sent {
                ctx.send(WorkerMsg::Eof {
                    worker_id: ctx.id,
                    generation: ctx.generation,
                    epoch,
                });
                eof_sent = true;
            }
            // Stay alive for seeks until cancelled.
            match ctx.rx.recv_timeout(Duration::from_millis(200)) {
                Ok(cmd) => carry = Some(cmd),
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::sine_wav;

    fn null_engine() -> AudioEngine {
        AudioEngine::with_config(EngineConfig {
            output: OutputMode::Null,
            idle_stream_timeout: Duration::from_millis(300),
            ..Default::default()
        })
    }

    fn wait_for(engine: &AudioEngine, timeout: Duration, f: impl Fn(&PlaybackSnapshot) -> bool) {
        let start = Instant::now();
        loop {
            let s = engine.snapshot();
            if f(&s) {
                return;
            }
            assert!(start.elapsed() < timeout, "timed out; last snapshot {s:?}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn tone(secs: f64) -> AudioSource {
        let frames = (44_100.0 * secs) as usize;
        AudioSource::bytes(sine_wav(44_100, 2, frames, 440.0), Some("wav"))
    }

    #[test]
    fn engine_is_send_sync_clone() {
        fn assert_traits<T: Send + Sync + Clone + 'static>() {}
        assert_traits::<AudioEngine>();
    }

    #[test]
    fn new_never_panics_and_reports_idle() {
        // Auto mode touches real hardware only lazily (on play), so this is safe headless.
        let e = AudioEngine::new();
        let s = e.snapshot();
        assert_eq!(s.state, PlaybackState::Idle);
        assert_eq!(s.track_generation, 0);
        assert!(!e.is_output_open());
        e.pause();
        e.resume();
        e.seek(3.0);
        e.stop();
        drop(e);
    }

    #[test]
    fn plays_to_end_and_emits_track_ended() {
        let e = null_engine();
        let mut rx = e.subscribe_events();
        let generation = e.play(tone(0.4), 0.0);
        assert_eq!(generation, 1);
        wait_for(&e, Duration::from_secs(5), |s| {
            s.state == PlaybackState::Ended
        });
        let s = e.snapshot();
        assert!((s.duration_secs - 0.4).abs() < 0.01, "{s:?}");
        assert!((s.position_secs - 0.4).abs() < 0.01, "{s:?}");
        let mut saw_playing = false;
        let mut ended = None;
        while let Ok(ev) = rx.try_recv() {
            match ev {
                AudioEvent::StateChanged(PlaybackState::Playing) => saw_playing = true,
                AudioEvent::TrackEnded { generation } => ended = Some(generation),
                _ => {}
            }
        }
        assert!(saw_playing);
        assert_eq!(ended, Some(1));
        // Output is released after the idle timeout.
        let start = Instant::now();
        while e.is_output_open() {
            assert!(
                start.elapsed() < Duration::from_secs(3),
                "stream not dropped"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn real_time_clock_pause_resume_seek_and_rate() {
        let e = null_engine();
        e.play(tone(10.0), 1.0);
        wait_for(&e, Duration::from_secs(5), |s| {
            s.state == PlaybackState::Playing
        });
        std::thread::sleep(Duration::from_millis(300));
        let p1 = e.snapshot().position_secs;
        assert!((1.15..1.6).contains(&p1), "position {p1}");

        e.pause();
        wait_for(&e, Duration::from_secs(2), |s| {
            s.state == PlaybackState::Paused
        });
        let paused_at = e.snapshot().position_secs;
        std::thread::sleep(Duration::from_millis(200));
        let after_pause = e.snapshot().position_secs;
        assert!(
            (after_pause - paused_at).abs() < 0.02,
            "clock drifted while paused: before={paused_at}, after={after_pause}"
        );

        e.seek(6.0);
        wait_for(&e, Duration::from_secs(2), |s| {
            (s.position_secs - 6.0).abs() < 1e-6
        });
        e.set_rate(2.0);
        e.resume();
        wait_for(&e, Duration::from_secs(2), |s| {
            s.state == PlaybackState::Playing
        });
        std::thread::sleep(Duration::from_millis(400));
        let p2 = e.snapshot().position_secs;
        // 2x speed: ~0.8 s of media in 0.4 s (allow ramp/latency slack).
        assert!((6.5..7.2).contains(&p2), "position at 2x {p2}");
        let s = e.snapshot();
        assert_eq!(s.rate, 2.0);

        e.set_volume(3.0);
        e.set_muted(true);
        let s = e.snapshot();
        assert_eq!((s.volume, s.muted), (1.0, true));
        e.stop();
        wait_for(&e, Duration::from_secs(2), |s| {
            s.state == PlaybackState::Stopped
        });
        assert!(!e.is_output_open());
    }

    #[test]
    fn toggle_and_resume_after_end_restarts() {
        let e = null_engine();
        e.play(tone(0.2), 0.0);
        wait_for(&e, Duration::from_secs(5), |s| {
            s.state == PlaybackState::Playing
        });
        e.toggle_play();
        wait_for(&e, Duration::from_secs(2), |s| {
            s.state == PlaybackState::Paused
        });
        e.toggle_play();
        wait_for(&e, Duration::from_secs(5), |s| {
            s.state == PlaybackState::Ended
        });
        e.toggle_play();
        wait_for(&e, Duration::from_secs(5), |s| {
            s.state == PlaybackState::Playing
        });
        wait_for(&e, Duration::from_secs(5), |s| {
            s.state == PlaybackState::Ended
        });
    }

    #[test]
    fn decode_error_reports_error_event() {
        let e = null_engine();
        e.set_ffmpeg_path(Some(PathBuf::from("/nonexistent/ffmpeg")));
        let mut rx = e.subscribe_events();
        let generation = e.play(AudioSource::bytes(vec![7u8; 2048], Some("xyz")), 0.0);
        wait_for(&e, Duration::from_secs(5), |s| {
            s.state == PlaybackState::Error
        });
        assert!(e.snapshot().error.is_some());
        let mut got = false;
        while let Ok(ev) = rx.try_recv() {
            if let AudioEvent::Error { generation: g, .. } = ev {
                assert_eq!(g, generation);
                got = true;
            }
        }
        assert!(got);
    }

    #[test]
    fn spectrum_follows_waveform_flag() {
        let e = null_engine();
        e.play(tone(3.0), 0.0);
        wait_for(&e, Duration::from_secs(5), |s| {
            s.state == PlaybackState::Playing
        });
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(
            e.spectrum(),
            [0.0; SPECTRUM_BARS],
            "disabled => no analysis"
        );
        e.set_waveform_enabled(true);
        let start = Instant::now();
        while e.spectrum().iter().all(|&b| b == 0.0) {
            assert!(
                start.elapsed() < Duration::from_secs(3),
                "spectrum never updated"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        let bars = e.spectrum();
        let peak = bars
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .unwrap()
            .0;
        let expected = SpectrumAnalyzer::new(DEFAULT_OUT_RATE)
            .bar_for_freq(440.0)
            .unwrap();
        assert!(
            (peak as i64 - expected as i64).abs() <= 1,
            "{peak} vs {expected}"
        );
        e.set_waveform_enabled(false);
        wait_for(&e, Duration::from_secs(2), |_| {
            e.spectrum() == [0.0; SPECTRUM_BARS]
        });
    }

    #[test]
    fn soak_rapid_20_track_plays_only_latest_ends() {
        let e = null_engine();
        let mut rx = e.subscribe_events();
        let sources: Vec<AudioSource> = (0..20).map(|i| tone(0.15 + i as f64 * 0.01)).collect();
        let mut last = 0;
        for (i, src) in sources.into_iter().enumerate() {
            last = e.play(src, 0.0);
            assert_eq!(last, i as u64 + 1);
            if i % 3 == 0 {
                std::thread::sleep(Duration::from_millis(2));
            }
            if i % 5 == 0 {
                e.seek(0.05);
                e.toggle_play();
                e.toggle_play();
            }
        }
        wait_for(&e, Duration::from_secs(10), |s| {
            s.state == PlaybackState::Ended
        });
        let mut ended = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            if let AudioEvent::TrackEnded { generation } = ev {
                ended.push(generation);
            }
        }
        assert_eq!(ended, vec![last], "only the latest generation may end");
        assert_eq!(e.snapshot().track_generation, 20);
        // Clones share the engine; dropping all of them joins the control thread cleanly.
        let clone = e.clone();
        drop(e);
        assert_eq!(clone.snapshot().track_generation, 20);
        drop(clone);
    }

    #[test]
    fn streaming_file_plays_while_downloading() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dl.wav");
        let wav = sine_wav(44_100, 2, 44_100 / 2, 440.0);
        std::fs::File::create(&path).unwrap();
        let src = AudioSource::streaming_file(&path, Some(wav.len() as u64));
        let AudioSource::StreamingFile { done, .. } = &src else {
            unreachable!()
        };
        let done = done.clone();
        let p2 = path.clone();
        let producer = std::thread::spawn(move || {
            let mut f = std::fs::OpenOptions::new().append(true).open(&p2).unwrap();
            for chunk in wav.chunks(8192) {
                f.write_all(chunk).unwrap();
                f.flush().unwrap();
                std::thread::sleep(Duration::from_millis(15));
            }
            done.store(true, Ordering::Release);
        });
        let e = null_engine();
        let mut rx = e.subscribe_events();
        e.play(src, 0.0);
        wait_for(&e, Duration::from_secs(10), |s| {
            s.state == PlaybackState::Ended
        });
        producer.join().unwrap();
        assert!((e.snapshot().position_secs - 0.5).abs() < 0.02);
        let mut ended = false;
        while let Ok(ev) = rx.try_recv() {
            ended |= matches!(ev, AudioEvent::TrackEnded { generation: 1 });
        }
        assert!(ended);
    }

    #[test]
    fn stale_track_end_never_adopts_newer_generation() {
        let e = null_engine();
        let mut rx = e.subscribe_events();
        let generation = e.play(tone(0.3), 0.0);
        wait_for(&e, Duration::from_secs(5), |s| {
            s.state == PlaybackState::Playing
        });
        // A new `play()` has bumped the shared counter but its `Cmd::Play` has not been
        // processed yet when the old track finishes.
        e.inner.shared.generation.fetch_add(1, Ordering::AcqRel);
        wait_for(&e, Duration::from_secs(5), |s| {
            s.state == PlaybackState::Ended
        });
        let mut ended = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            if let AudioEvent::TrackEnded { generation } = ev {
                ended.push(generation);
            }
        }
        assert_eq!(ended, vec![generation]);
    }

    #[test]
    fn stale_error_never_adopts_newer_generation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stale.wav");
        let wav = sine_wav(44_100, 2, 44_100 * 5, 440.0);
        std::fs::write(&path, &wav[..wav.len() / 10]).unwrap();
        let src = AudioSource::streaming_file(&path, Some(wav.len() as u64));
        let AudioSource::StreamingFile { aborted, .. } = &src else {
            unreachable!()
        };
        let aborted = aborted.clone();
        let e = null_engine();
        let mut rx = e.subscribe_events();
        let generation = e.play(src, 0.0);
        wait_for(&e, Duration::from_secs(5), |s| {
            s.state == PlaybackState::Playing
        });
        e.inner.shared.generation.fetch_add(1, Ordering::AcqRel);
        aborted.store(true, Ordering::Release);
        wait_for(&e, Duration::from_secs(5), |s| {
            s.state == PlaybackState::Error
        });
        let mut errors = Vec::new();
        while let Ok(ev) = rx.try_recv() {
            if let AudioEvent::Error { generation, .. } = ev {
                errors.push(generation);
            }
        }
        assert_eq!(errors, vec![generation]);
    }

    #[test]
    fn stop_interrupts_blocked_streaming_read_promptly() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stalled.wav");
        let wav = sine_wav(44_100, 2, 44_100 * 5, 440.0);
        // Only ~0.25 s downloaded and no more bytes ever arrive (idle timeout is 30 s).
        std::fs::write(&path, &wav[..wav.len() / 20]).unwrap();
        let src = AudioSource::streaming_file(&path, Some(wav.len() as u64));
        let AudioSource::StreamingFile { done, .. } = &src else {
            unreachable!()
        };
        let done = done.clone();
        let e = null_engine();
        e.play(src, 0.0);
        wait_for(&e, Duration::from_secs(5), |s| {
            s.state == PlaybackState::Playing
        });
        // Let the worker drain what exists and block in the growing-file wait loop.
        std::thread::sleep(Duration::from_millis(400));
        // Holders: this test, Control::source, the worker's ctx and the open media source.
        assert!(
            Arc::strong_count(&done) >= 3,
            "{}",
            Arc::strong_count(&done)
        );
        e.stop();
        let stopped = Instant::now();
        // Only this test and Control::source remain once the worker thread has exited.
        while Arc::strong_count(&done) > 2 {
            assert!(
                stopped.elapsed() < Duration::from_millis(200),
                "worker still blocked after stop ({} holders)",
                Arc::strong_count(&done)
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    #[test]
    fn output_rate_change_on_reopen_flushes_stale_rate_audio() {
        let e = null_engine();
        e.play(tone(10.0), 0.0);
        wait_for(&e, Duration::from_secs(5), |s| {
            s.state == PlaybackState::Playing
        });
        std::thread::sleep(Duration::from_millis(100));
        e.pause();
        wait_for(&e, Duration::from_secs(2), |s| {
            s.state == PlaybackState::Paused
        });
        // Idle teardown; the ring stays full of 48 kHz audio meanwhile.
        let start = Instant::now();
        while e.is_output_open() {
            assert!(
                start.elapsed() < Duration::from_secs(3),
                "stream not dropped"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        e.send(Cmd::SetNullRate(22_050));
        let p0 = e.snapshot().position_secs;
        e.resume();
        wait_for(&e, Duration::from_secs(2), |s| {
            s.state == PlaybackState::Playing
        });
        assert_eq!(e.inner.shared.out_rate.load(Ordering::Acquire), 22_050);
        std::thread::sleep(Duration::from_millis(400));
        let advanced = e.snapshot().position_secs - p0;
        // Stale 48 kHz samples played at 22.05 kHz would advance only ~0.18 s.
        assert!((0.3..0.5).contains(&advanced), "advanced {advanced}");
    }

    /// Opens the real default output device (muted). Run with `--ignored` on a dev machine.
    #[test]
    #[ignore = "touches real audio hardware"]
    fn real_device_smoke_muted() {
        let e = AudioEngine::with_config(EngineConfig {
            idle_stream_timeout: Duration::from_millis(300),
            ..Default::default()
        });
        e.set_muted(true);
        e.play(tone(0.6), 0.0);
        wait_for(&e, Duration::from_secs(5), |s| {
            s.state == PlaybackState::Playing
        });
        let s = e.snapshot();
        eprintln!("device snapshot while playing: {s:?}");
        assert!(e.is_output_open());
        wait_for(&e, Duration::from_secs(5), |s| {
            s.state == PlaybackState::Ended
        });
        let start = Instant::now();
        while e.is_output_open() {
            assert!(
                start.elapsed() < Duration::from_secs(3),
                "device stream not released"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        eprintln!(
            "device stream released after idle; has_audio_device={}",
            s.has_audio_device
        );
    }

    #[test]
    fn aborted_streaming_file_errors_instead_of_ending() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("abort.wav");
        let wav = sine_wav(44_100, 2, 44_100 * 5, 440.0);
        std::fs::write(&path, &wav[..wav.len() / 10]).unwrap();
        let src = AudioSource::streaming_file(&path, Some(wav.len() as u64));
        let AudioSource::StreamingFile { aborted, .. } = &src else {
            unreachable!()
        };
        let aborted = aborted.clone();
        let e = null_engine();
        let mut rx = e.subscribe_events();
        e.play(src, 0.0);
        wait_for(&e, Duration::from_secs(5), |s| {
            s.state == PlaybackState::Playing
        });
        aborted.store(true, Ordering::Release);
        wait_for(&e, Duration::from_secs(5), |s| {
            s.state == PlaybackState::Error
        });
        assert!(e.snapshot().error.unwrap().contains("aborted"));
        while let Ok(ev) = rx.try_recv() {
            assert!(!matches!(ev, AudioEvent::TrackEnded { .. }));
        }
    }
}
