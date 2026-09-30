//! L-Player core: owns a libmpv instance, renders its video frames into a
//! Flutter texture (software rendering + irondash_texture) and streams the
//! playback state back to Dart.
//!
//! Threads:
//! - event thread: blocks on `mpv_wait_event`, keeps [`PlayerState`] up to date
//!   and pushes [`PlayerEvent`]s to Dart.
//! - render thread: woken by mpv's render update callback, renders the frame
//!   into a pixel buffer and tells Flutter a new frame is available.

use std::ffi::{c_char, c_int, c_void, CStr, CString};
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use anyhow::{anyhow, bail, Result};
use flutter_rust_bridge::frb;
use irondash_run_loop::RunLoop;
use irondash_texture::{
    BoxedPixelData, PayloadProvider, PixelData, PixelDataProvider, PixelFormat, SendableTexture,
    SimplePixelData, Texture,
};

use crate::frb_generated::StreamSink;
use crate::mpv::*;

/// Frames bigger than this are downscaled by mpv to keep the CPU copy cheap.
const MAX_FRAME_WIDTH: f64 = 1920.0;
const MAX_FRAME_HEIGHT: f64 = 1080.0;

/// How many spare frame buffers the render thread keeps for reuse.
const FRAME_POOL_SIZE: usize = 3;

#[frb(init)]
pub fn init_app() {
    flutter_rust_bridge::setup_default_user_utils();
}

/// Snapshot of everything the UI needs to draw the player.
#[derive(Debug, Clone)]
pub struct PlayerState {
    /// Current position in seconds.
    pub position: f64,
    /// Duration in seconds, 0 when unknown (e.g. live streams).
    pub duration: f64,
    pub paused: bool,
    /// True when nothing is loaded (initial state, after stop or end of file).
    pub idle: bool,
    /// True while a network stream is waiting for data.
    pub buffering: bool,
    /// 0..=100
    pub volume: f64,
    pub speed: f64,
    pub subtitles_visible: bool,
    /// Display size of the video, 0 for audio only files.
    pub video_width: i32,
    pub video_height: i32,
    pub title: String,
    /// Audio, video and subtitle tracks of the current file.
    pub tracks: Vec<MediaTrack>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackKind {
    Video,
    Audio,
    Subtitle,
}

#[derive(Debug, Clone)]
pub struct MediaTrack {
    /// mpv track id, unique per kind. Pass it to `select_audio_track` or
    /// `select_subtitle_track`.
    pub id: i32,
    pub kind: TrackKind,
    pub title: Option<String>,
    /// Language code, e.g. "eng" or "jpn".
    pub language: Option<String>,
    pub codec: Option<String>,
    pub selected: bool,
    /// Path of an external subtitle/audio file, None for embedded tracks.
    pub external_filename: Option<String>,
}

impl Default for PlayerState {
    fn default() -> Self {
        Self {
            position: 0.0,
            duration: 0.0,
            paused: false,
            idle: true,
            buffering: false,
            volume: 100.0,
            speed: 1.0,
            subtitles_visible: true,
            video_width: 0,
            video_height: 0,
            title: String::new(),
            tracks: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlayerEventKind {
    StateChanged,
    FileLoaded,
    /// The current file played until the end, the UI should advance the playlist.
    EndOfFile,
    /// The current file failed to load or play, `message` has the reason.
    Error,
}

#[derive(Debug, Clone)]
pub struct PlayerEvent {
    pub kind: PlayerEventKind,
    pub state: PlayerState,
    pub message: Option<String>,
}

#[frb(opaque)]
pub struct LPlayer {
    inner: Arc<Inner>,
    texture_id: i64,
    threads: Mutex<Vec<JoinHandle<()>>>,
}

/// Creates the player and its video texture. `engine_handle` comes from
/// `EngineContext.instance.getEngineHandle()` on the Dart side.
pub fn create_player(engine_handle: i64) -> Result<LPlayer> {
    let main_thread = RunLoop::sender_for_main_thread()
        .map_err(|e| anyhow!("main thread is not available: {e:?}"))?;

    // libmpv refuses to start unless numbers use the C locale.
    main_thread.send_and_wait(|| unsafe {
        libc::setlocale(libc::LC_NUMERIC, c"C".as_ptr());
    });
    let mpv = Mpv::new()?;
    let render_ctx = mpv.create_render_context()?;

    // Flutter textures have to be created on the platform (main) thread.
    let frames = Arc::new(FrameProvider::new());
    let provider = frames.clone();
    let (texture_id, texture) = main_thread
        .send_and_wait(move || {
            Texture::<BoxedPixelData>::new_with_provider(engine_handle, provider)
                .map(|texture| (texture.id(), texture.into_sendable_texture()))
                .map_err(|e| e.to_string())
        })
        .map_err(|e| anyhow!("failed to create video texture: {e}"))?;
    let texture = MainThreadDrop::new(texture);

    let inner = Arc::new(Inner {
        mpv,
        shutdown: AtomicBool::new(false),
        state: Mutex::new(PlayerState::default()),
        sink: Mutex::new(None),
        pending_subtitles: Mutex::new(Vec::new()),
        file_loaded: AtomicBool::new(false),
        redraw: Mutex::new(false),
        redraw_cv: Condvar::new(),
        force_redraw: AtomicBool::new(false),
        frames,
        texture,
    });

    let render_thread = {
        let inner = inner.clone();
        std::thread::Builder::new()
            .name("lplayer-render".into())
            .spawn(move || render_loop(inner, render_ctx))?
    };
    let event_thread = {
        let inner = inner.clone();
        std::thread::Builder::new()
            .name("lplayer-events".into())
            .spawn(move || event_loop(inner))?
    };

    Ok(LPlayer {
        inner,
        texture_id,
        threads: Mutex::new(vec![render_thread, event_thread]),
    })
}

impl LPlayer {
    /// Id for Flutter's `Texture(textureId: ...)` widget.
    #[frb(sync, getter)]
    pub fn texture_id(&self) -> i64 {
        self.texture_id
    }

    /// Stream of playback events. Emits the current state right away.
    pub fn events(&self, sink: StreamSink<PlayerEvent>) -> Result<()> {
        *self.inner.sink.lock().unwrap() = Some(sink);
        self.inner.emit(PlayerEventKind::StateChanged, None);
        Ok(())
    }

    /// Plays a local file or URL, replacing whatever is playing now.
    /// `subtitles` are external subtitle files added once the file is loaded.
    pub fn open(&self, uri: String, subtitles: Vec<String>) -> Result<()> {
        *self.inner.pending_subtitles.lock().unwrap() = subtitles;
        self.inner.command(&["loadfile", &uri, "replace"])?;
        self.inner.set_flag("pause", false)
    }

    pub fn play(&self) -> Result<()> {
        self.inner.set_flag("pause", false)
    }

    pub fn pause(&self) -> Result<()> {
        self.inner.set_flag("pause", true)
    }

    pub fn toggle_pause(&self) -> Result<()> {
        self.inner.command(&["cycle", "pause"])
    }

    pub fn stop(&self) -> Result<()> {
        self.inner.pending_subtitles.lock().unwrap().clear();
        self.inner.command(&["stop"])
    }

    /// Seeks to an absolute position in seconds.
    pub fn seek_to(&self, seconds: f64) -> Result<()> {
        if !self.inner.file_loaded.load(Ordering::Acquire) {
            return Ok(());
        }
        self.inner
            .command(&["seek", &seconds.to_string(), "absolute"])
    }

    /// Seeks relative to the current position, negative goes backward.
    /// Uses exact seeking so small steps like 0.5s are honored.
    pub fn seek_by(&self, seconds: f64) -> Result<()> {
        if !self.inner.file_loaded.load(Ordering::Acquire) {
            return Ok(());
        }
        self.inner
            .command(&["seek", &seconds.to_string(), "relative+exact"])
    }

    /// Volume in 0..=100.
    pub fn set_volume(&self, volume: f64) -> Result<()> {
        self.inner.set_double("volume", volume.clamp(0.0, 100.0))
    }

    /// Playback speed, 1.0 is normal.
    pub fn set_speed(&self, speed: f64) -> Result<()> {
        self.inner.set_double("speed", speed.clamp(0.1, 8.0))
    }

    pub fn set_subtitles_visible(&self, visible: bool) -> Result<()> {
        self.inner.set_flag("sub-visibility", visible)
    }

    /// Selects an audio track by id, None turns audio off.
    pub fn select_audio_track(&self, id: Option<i32>) -> Result<()> {
        self.inner.set_string("aid", &track_value(id))
    }

    /// Selects a subtitle track by id, None turns subtitles off.
    pub fn select_subtitle_track(&self, id: Option<i32>) -> Result<()> {
        self.inner.set_string("sid", &track_value(id))
    }

    /// Adds an external subtitle to the current file (or to the file that is
    /// being loaded right now) and selects it.
    pub fn add_subtitle(&self, path: String) -> Result<()> {
        let mut pending = self.inner.pending_subtitles.lock().unwrap();
        if self.inner.file_loaded.load(Ordering::Acquire) {
            drop(pending);
            self.inner.command(&["sub-add", &path, "select"])
        } else {
            pending.push(path);
            Ok(())
        }
    }
}

fn track_value(id: Option<i32>) -> String {
    id.map_or_else(|| "no".to_string(), |id| id.to_string())
}

impl Drop for LPlayer {
    fn drop(&mut self) {
        self.inner.shutdown.store(true, Ordering::Release);
        *self.inner.redraw.lock().unwrap() = true;
        self.inner.redraw_cv.notify_all();
        unsafe { mpv_wakeup(self.inner.mpv.0) };
        for thread in self.threads.get_mut().unwrap().drain(..) {
            let _ = thread.join();
        }
        // mpv itself is destroyed when the last `Arc<Inner>` goes away, which
        // happens after the render thread freed its render context.
    }
}

// ---------------------------------------------------------------------------
// mpv handle
// ---------------------------------------------------------------------------

/// Owned mpv handle. The client API is thread safe, so sharing is fine.
struct Mpv(*mut mpv_handle);

unsafe impl Send for Mpv {}
unsafe impl Sync for Mpv {}

impl Mpv {
    fn new() -> Result<Self> {
        let handle = unsafe { mpv_create() };
        if handle.is_null() {
            bail!("mpv_create failed");
        }
        let mpv = Mpv(handle);

        for (name, value) in [
            // Render through the render API instead of opening a window.
            ("vo", "libmpv"),
            ("hwdec", "auto-copy"),
            ("idle", "yes"),
            ("keep-open", "no"),
            ("osc", "no"),
            ("terminal", "no"),
            ("input-default-bindings", "no"),
            ("input-vo-keyboard", "no"),
            // Pick up subtitles next to the video with a similar name.
            ("sub-auto", "fuzzy"),
        ] {
            mpv.set_option(name, value)?;
        }
        check(unsafe { mpv_initialize(mpv.0) }, "mpv_initialize")?;

        for (id, name, format) in OBSERVED_PROPERTIES {
            check(
                unsafe { mpv_observe_property(mpv.0, *id, name.as_ptr(), *format) },
                "mpv_observe_property",
            )?;
        }
        Ok(mpv)
    }

    fn create_render_context(&self) -> Result<RenderContext> {
        let mut params = [
            mpv_render_param {
                type_: MPV_RENDER_PARAM_API_TYPE,
                data: MPV_RENDER_API_TYPE_SW.as_ptr() as *mut c_void,
            },
            mpv_render_param {
                type_: MPV_RENDER_PARAM_INVALID,
                data: ptr::null_mut(),
            },
        ];
        let mut ctx: *mut mpv_render_context = ptr::null_mut();
        check(
            unsafe { mpv_render_context_create(&mut ctx, self.0, params.as_mut_ptr()) },
            "mpv_render_context_create",
        )?;
        Ok(RenderContext(ctx))
    }

    fn set_option(&self, name: &str, value: &str) -> Result<()> {
        let c_name = CString::new(name)?;
        let c_value = CString::new(value)?;
        check(
            unsafe { mpv_set_option_string(self.0, c_name.as_ptr(), c_value.as_ptr()) },
            name,
        )
    }
}

impl Drop for Mpv {
    fn drop(&mut self) {
        unsafe { mpv_terminate_destroy(self.0) };
    }
}

/// Render context, only ever touched by the render thread after creation.
/// Must be dropped before the [`Mpv`] it was created from.
struct RenderContext(*mut mpv_render_context);

unsafe impl Send for RenderContext {}

impl Drop for RenderContext {
    fn drop(&mut self) {
        unsafe {
            mpv_render_context_set_update_callback(self.0, None, ptr::null_mut());
            mpv_render_context_free(self.0);
        }
    }
}

/// Drops the wrapped value on the platform thread. irondash textures panic
/// when their last reference goes away on any other thread.
struct MainThreadDrop<T: Send + 'static>(Option<T>);

impl<T: Send + 'static> MainThreadDrop<T> {
    fn new(value: T) -> Self {
        Self(Some(value))
    }
}

impl<T: Send + 'static> std::ops::Deref for MainThreadDrop<T> {
    type Target = T;

    fn deref(&self) -> &T {
        self.0.as_ref().expect("value is only taken on drop")
    }
}

impl<T: Send + 'static> Drop for MainThreadDrop<T> {
    fn drop(&mut self) {
        let Some(value) = self.0.take() else {
            return;
        };
        match RunLoop::sender_for_main_thread() {
            Ok(sender) => sender.send(move || drop(value)),
            // Leaking beats panicking while the engine is going away.
            Err(_) => std::mem::forget(value),
        }
    }
}

fn check(code: c_int, what: &str) -> Result<()> {
    if code < 0 {
        bail!("{what}: {}", error_string(code));
    }
    Ok(())
}

fn error_string(code: c_int) -> String {
    unsafe { CStr::from_ptr(mpv_error_string(code)) }
        .to_string_lossy()
        .into_owned()
}

const PROP_TIME_POS: u64 = 1;
const PROP_DURATION: u64 = 2;
const PROP_PAUSE: u64 = 3;
const PROP_IDLE: u64 = 4;
const PROP_BUFFERING: u64 = 5;
const PROP_VOLUME: u64 = 6;
const PROP_SPEED: u64 = 7;
const PROP_SUB_VISIBILITY: u64 = 8;
const PROP_DWIDTH: u64 = 9;
const PROP_DHEIGHT: u64 = 10;
const PROP_MEDIA_TITLE: u64 = 11;
// Observed without a value, any change refreshes `PlayerState::tracks`.
const PROP_TRACK_LIST: u64 = 12;
const PROP_AID: u64 = 13;
const PROP_SID: u64 = 14;

const OBSERVED_PROPERTIES: &[(u64, &CStr, c_int)] = &[
    (PROP_TIME_POS, c"time-pos", MPV_FORMAT_DOUBLE),
    (PROP_DURATION, c"duration", MPV_FORMAT_DOUBLE),
    (PROP_PAUSE, c"pause", MPV_FORMAT_FLAG),
    (PROP_IDLE, c"idle-active", MPV_FORMAT_FLAG),
    (PROP_BUFFERING, c"paused-for-cache", MPV_FORMAT_FLAG),
    (PROP_VOLUME, c"volume", MPV_FORMAT_DOUBLE),
    (PROP_SPEED, c"speed", MPV_FORMAT_DOUBLE),
    (PROP_SUB_VISIBILITY, c"sub-visibility", MPV_FORMAT_FLAG),
    (PROP_DWIDTH, c"dwidth", MPV_FORMAT_INT64),
    (PROP_DHEIGHT, c"dheight", MPV_FORMAT_INT64),
    (PROP_MEDIA_TITLE, c"media-title", MPV_FORMAT_STRING),
    (PROP_TRACK_LIST, c"track-list", MPV_FORMAT_NONE),
    (PROP_AID, c"aid", MPV_FORMAT_NONE),
    (PROP_SID, c"sid", MPV_FORMAT_NONE),
];

// ---------------------------------------------------------------------------
// Shared player state
// ---------------------------------------------------------------------------

struct Inner {
    mpv: Mpv,
    shutdown: AtomicBool,
    state: Mutex<PlayerState>,
    sink: Mutex<Option<StreamSink<PlayerEvent>>>,
    /// External subtitles to add once the file being loaded is ready.
    pending_subtitles: Mutex<Vec<String>>,
    file_loaded: AtomicBool,
    /// Set by mpv's update callback (or on shutdown) to wake the render thread.
    redraw: Mutex<bool>,
    redraw_cv: Condvar,
    /// Render even if mpv reports no new frame, e.g. after the video size became known.
    force_redraw: AtomicBool,
    frames: Arc<FrameProvider>,
    texture: MainThreadDrop<Arc<SendableTexture<BoxedPixelData>>>,
}

impl Inner {
    fn command(&self, args: &[&str]) -> Result<()> {
        let c_args = args
            .iter()
            .map(|arg| CString::new(*arg))
            .collect::<Result<Vec<_>, _>>()?;
        let mut ptrs: Vec<*const c_char> = c_args.iter().map(|arg| arg.as_ptr()).collect();
        ptrs.push(ptr::null());
        check(
            unsafe { mpv_command(self.mpv.0, ptrs.as_mut_ptr()) },
            args[0],
        )
    }

    fn set_double(&self, name: &str, mut value: f64) -> Result<()> {
        let c_name = CString::new(name)?;
        check(
            unsafe {
                mpv_set_property(
                    self.mpv.0,
                    c_name.as_ptr(),
                    MPV_FORMAT_DOUBLE,
                    &mut value as *mut f64 as *mut c_void,
                )
            },
            name,
        )
    }

    fn set_flag(&self, name: &str, value: bool) -> Result<()> {
        let c_name = CString::new(name)?;
        let mut flag: c_int = value.into();
        check(
            unsafe {
                mpv_set_property(
                    self.mpv.0,
                    c_name.as_ptr(),
                    MPV_FORMAT_FLAG,
                    &mut flag as *mut c_int as *mut c_void,
                )
            },
            name,
        )
    }

    fn set_string(&self, name: &str, value: &str) -> Result<()> {
        let c_name = CString::new(name)?;
        let c_value = CString::new(value)?;
        check(
            unsafe { mpv_set_property_string(self.mpv.0, c_name.as_ptr(), c_value.as_ptr()) },
            name,
        )
    }

    fn get_string(&self, name: &str) -> Option<String> {
        let c_name = CString::new(name).ok()?;
        let value = unsafe { mpv_get_property_string(self.mpv.0, c_name.as_ptr()) };
        if value.is_null() {
            return None;
        }
        let result = unsafe { CStr::from_ptr(value) }
            .to_string_lossy()
            .into_owned();
        unsafe { mpv_free(value as *mut c_void) };
        Some(result)
    }

    /// Re-reads `track-list`. Only call from the event thread, never while
    /// holding the state lock.
    fn refresh_tracks(&self) {
        let count: i32 = self
            .get_string("track-list/count")
            .and_then(|count| count.parse().ok())
            .unwrap_or(0);
        let tracks = (0..count)
            .filter_map(|index| {
                let field = |name: &str| self.get_string(&format!("track-list/{index}/{name}"));
                let kind = match field("type")?.as_str() {
                    "video" => TrackKind::Video,
                    "audio" => TrackKind::Audio,
                    "sub" => TrackKind::Subtitle,
                    _ => return None,
                };
                Some(MediaTrack {
                    id: field("id")?.parse().ok()?,
                    kind,
                    title: field("title"),
                    language: field("lang"),
                    codec: field("codec"),
                    selected: field("selected").as_deref() == Some("yes"),
                    external_filename: field("external-filename"),
                })
            })
            .collect();
        self.state.lock().unwrap().tracks = tracks;
    }

    fn emit(&self, kind: PlayerEventKind, message: Option<String>) {
        let state = self.state.lock().unwrap().clone();
        if let Some(sink) = self.sink.lock().unwrap().as_ref() {
            let _ = sink.add(PlayerEvent {
                kind,
                state,
                message,
            });
        }
    }

    fn request_redraw(&self, force: bool) {
        if force {
            self.force_redraw.store(true, Ordering::Release);
        }
        *self.redraw.lock().unwrap() = true;
        self.redraw_cv.notify_one();
    }

    /// Applies an observed property change, returns false for unknown ids.
    fn apply_property(&self, id: u64, prop: &mpv_event_property) -> bool {
        let mut state = self.state.lock().unwrap();
        match id {
            PROP_TIME_POS => state.position = prop_double(prop).unwrap_or(0.0),
            PROP_DURATION => state.duration = prop_double(prop).unwrap_or(0.0),
            PROP_PAUSE => state.paused = prop_flag(prop).unwrap_or(false),
            PROP_IDLE => state.idle = prop_flag(prop).unwrap_or(true),
            PROP_BUFFERING => state.buffering = prop_flag(prop).unwrap_or(false),
            PROP_VOLUME => state.volume = prop_double(prop).unwrap_or(state.volume),
            PROP_SPEED => state.speed = prop_double(prop).unwrap_or(state.speed),
            PROP_SUB_VISIBILITY => {
                state.subtitles_visible = prop_flag(prop).unwrap_or(state.subtitles_visible)
            }
            PROP_DWIDTH | PROP_DHEIGHT => {
                let value = prop_int64(prop).unwrap_or(0) as i32;
                if id == PROP_DWIDTH {
                    state.video_width = value;
                } else {
                    state.video_height = value;
                }
                drop(state);
                // The first frame may have been skipped while the size was unknown.
                self.request_redraw(true);
            }
            PROP_MEDIA_TITLE => state.title = prop_string(prop).unwrap_or_default(),
            _ => return false,
        }
        true
    }
}

fn prop_double(prop: &mpv_event_property) -> Option<f64> {
    (prop.format == MPV_FORMAT_DOUBLE && !prop.data.is_null())
        .then(|| unsafe { *(prop.data as *const f64) })
}

fn prop_flag(prop: &mpv_event_property) -> Option<bool> {
    (prop.format == MPV_FORMAT_FLAG && !prop.data.is_null())
        .then(|| unsafe { *(prop.data as *const c_int) } != 0)
}

fn prop_int64(prop: &mpv_event_property) -> Option<i64> {
    (prop.format == MPV_FORMAT_INT64 && !prop.data.is_null())
        .then(|| unsafe { *(prop.data as *const i64) })
}

fn prop_string(prop: &mpv_event_property) -> Option<String> {
    if prop.format != MPV_FORMAT_STRING || prop.data.is_null() {
        return None;
    }
    let value = unsafe { *(prop.data as *const *const c_char) };
    (!value.is_null()).then(|| {
        unsafe { CStr::from_ptr(value) }
            .to_string_lossy()
            .into_owned()
    })
}

// ---------------------------------------------------------------------------
// Event thread
// ---------------------------------------------------------------------------

fn event_loop(inner: Arc<Inner>) {
    loop {
        let event = unsafe { &*mpv_wait_event(inner.mpv.0, -1.0) };
        if inner.shutdown.load(Ordering::Acquire) {
            break;
        }
        match event.event_id {
            MPV_EVENT_SHUTDOWN => break,
            MPV_EVENT_PROPERTY_CHANGE => {
                let prop = unsafe { &*(event.data as *const mpv_event_property) };
                let changed = match event.reply_userdata {
                    PROP_TRACK_LIST | PROP_AID | PROP_SID => {
                        inner.refresh_tracks();
                        true
                    }
                    id => inner.apply_property(id, prop),
                };
                if changed {
                    inner.emit(PlayerEventKind::StateChanged, None);
                }
            }
            MPV_EVENT_START_FILE => inner.file_loaded.store(false, Ordering::Release),
            MPV_EVENT_FILE_LOADED => {
                let subtitles = {
                    let mut pending = inner.pending_subtitles.lock().unwrap();
                    inner.file_loaded.store(true, Ordering::Release);
                    std::mem::take(&mut *pending)
                };
                for path in subtitles {
                    if let Err(e) = inner.command(&["sub-add", &path, "select"]) {
                        inner.emit(PlayerEventKind::Error, Some(e.to_string()));
                    }
                }
                inner.emit(PlayerEventKind::FileLoaded, None);
            }
            MPV_EVENT_END_FILE => {
                inner.file_loaded.store(false, Ordering::Release);
                let end = unsafe { &*(event.data as *const mpv_event_end_file) };
                match end.reason {
                    MPV_END_FILE_REASON_EOF => inner.emit(PlayerEventKind::EndOfFile, None),
                    MPV_END_FILE_REASON_ERROR => {
                        inner.emit(PlayerEventKind::Error, Some(error_string(end.error)))
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

// ---------------------------------------------------------------------------
// Render thread
// ---------------------------------------------------------------------------

unsafe extern "C" fn on_render_update(ctx: *mut c_void) {
    // Must not call into mpv here, just wake the render thread.
    let inner = &*(ctx as *const Inner);
    inner.request_redraw(false);
}

fn render_loop(inner: Arc<Inner>, ctx: RenderContext) {
    let pixel_format: &CStr = match PixelData::FORMAT {
        PixelFormat::RGBA => c"rgb0",
        PixelFormat::BGRA => c"bgr0",
    };
    // `inner` outlives the callback: the context is dropped at the end of this
    // function, before this thread lets go of its Arc.
    unsafe {
        mpv_render_context_set_update_callback(
            ctx.0,
            Some(on_render_update),
            Arc::as_ptr(&inner) as *mut c_void,
        )
    };

    let mut pool: Vec<Arc<Vec<u8>>> = Vec::new();
    loop {
        {
            let mut redraw = inner.redraw.lock().unwrap();
            while !*redraw {
                redraw = inner.redraw_cv.wait(redraw).unwrap();
            }
            *redraw = false;
        }
        if inner.shutdown.load(Ordering::Acquire) {
            break;
        }

        // Has to be called on every wakeup, even if we end up not rendering.
        let flags = unsafe { mpv_render_context_update(ctx.0) };
        let forced = inner.force_redraw.swap(false, Ordering::AcqRel);
        if flags & MPV_RENDER_UPDATE_FRAME == 0 && !forced {
            continue;
        }

        let size = {
            let state = inner.state.lock().unwrap();
            frame_size(state.video_width, state.video_height)
        };
        let Some((width, height)) = size else {
            continue;
        };

        let mut buffer = take_buffer(&mut pool, (width * height * 4) as usize);
        let pixels = Arc::get_mut(&mut buffer).expect("buffer from pool is unique");

        let mut sw_size: [c_int; 2] = [width, height];
        let mut stride: usize = width as usize * 4;
        let mut params = [
            mpv_render_param {
                type_: MPV_RENDER_PARAM_SW_SIZE,
                data: sw_size.as_mut_ptr() as *mut c_void,
            },
            mpv_render_param {
                type_: MPV_RENDER_PARAM_SW_FORMAT,
                data: pixel_format.as_ptr() as *mut c_void,
            },
            mpv_render_param {
                type_: MPV_RENDER_PARAM_SW_STRIDE,
                data: &mut stride as *mut usize as *mut c_void,
            },
            mpv_render_param {
                type_: MPV_RENDER_PARAM_SW_POINTER,
                data: pixels.as_mut_ptr() as *mut c_void,
            },
            mpv_render_param {
                type_: MPV_RENDER_PARAM_INVALID,
                data: ptr::null_mut(),
            },
        ];
        if unsafe { mpv_render_context_render(ctx.0, params.as_mut_ptr()) } < 0 {
            pool.push(buffer);
            continue;
        }

        // "rgb0" leaves the 4th byte undefined, Flutter needs it opaque.
        for pixel in pixels.chunks_exact_mut(4) {
            pixel[3] = 255;
        }

        if let Some(previous) = inner.frames.publish(Frame {
            width,
            height,
            data: buffer,
        }) {
            if pool.len() < FRAME_POOL_SIZE {
                pool.push(previous);
            }
        }
        inner.texture.mark_frame_available();
    }

    // Free the render context while `inner` (and with it mpv) is still alive.
    drop(ctx);
    drop(inner);
}

/// Size to render a `video_width`x`video_height` video at, or None if unknown.
/// Width is kept a multiple of 16 so rows stay 64 byte aligned for mpv.
fn frame_size(video_width: i32, video_height: i32) -> Option<(i32, i32)> {
    if video_width <= 0 || video_height <= 0 {
        return None;
    }
    let (w, h) = (video_width as f64, video_height as f64);
    let scale = (MAX_FRAME_WIDTH / w).min(MAX_FRAME_HEIGHT / h).min(1.0);
    let width = (((w * scale) as i32) / 16 * 16).max(16);
    let height = ((h * scale).round() as i32).max(2);
    Some((width, height))
}

/// Returns a buffer of `len` bytes nobody else references, reusing the pool when possible.
fn take_buffer(pool: &mut Vec<Arc<Vec<u8>>>, len: usize) -> Arc<Vec<u8>> {
    pool.retain(|buffer| buffer.len() == len);
    match pool
        .iter_mut()
        .position(|buffer| Arc::get_mut(buffer).is_some())
    {
        Some(index) => pool.swap_remove(index),
        None => Arc::new(vec![0; len]),
    }
}

// ---------------------------------------------------------------------------
// Texture payload
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Frame {
    width: i32,
    height: i32,
    data: Arc<Vec<u8>>,
}

impl PixelDataProvider for Frame {
    fn get(&self) -> PixelData<'_> {
        PixelData {
            width: self.width,
            height: self.height,
            data: &self.data,
        }
    }
}

/// Hands the latest rendered frame to Flutter's raster thread.
struct FrameProvider {
    current: Mutex<Option<Frame>>,
}

impl FrameProvider {
    fn new() -> Self {
        Self {
            current: Mutex::new(None),
        }
    }

    /// Stores a new frame and returns the previous frame's buffer.
    fn publish(&self, frame: Frame) -> Option<Arc<Vec<u8>>> {
        self.current
            .lock()
            .unwrap()
            .replace(frame)
            .map(|previous| previous.data)
    }
}

impl PayloadProvider<BoxedPixelData> for FrameProvider {
    fn get_payload(&self) -> BoxedPixelData {
        match self.current.lock().unwrap().as_ref() {
            Some(frame) => Box::new(frame.clone()),
            None => SimplePixelData::new_boxed(1, 1, vec![0, 0, 0, 255]),
        }
    }
}
