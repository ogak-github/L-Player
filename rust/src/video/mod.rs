//! Video output: a render thread draws mpv's frames into a Flutter texture.
//!
//! Linux renders on the GPU (OpenGL ES through EGL, see [`gl`]). Everything
//! else, and Linux setups where the GPU path can't start, use the software
//! renderer in [`sw`]. Set `LPLAYER_RENDERER=sw` to force software rendering.

#[cfg(target_os = "linux")]
mod egl;
#[cfg(target_os = "linux")]
mod gl;
mod sw;

use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;

use anyhow::Result;
use irondash_run_loop::RunLoop;

use crate::mpv::*;

/// State shared between the player and the render thread.
pub(crate) struct VideoShared {
    shutdown: AtomicBool,
    /// Set by mpv's update callback (or on shutdown) to wake the render thread.
    redraw: Mutex<bool>,
    redraw_cv: Condvar,
    /// Render even if mpv reports no new frame, e.g. after the video size became known.
    force_redraw: AtomicBool,
    /// Display size of the video, 0 while unknown or for audio only files.
    size: Mutex<(i32, i32)>,
}

impl VideoShared {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            shutdown: AtomicBool::new(false),
            redraw: Mutex::new(false),
            redraw_cv: Condvar::new(),
            force_redraw: AtomicBool::new(false),
            size: Mutex::new((0, 0)),
        })
    }

    pub(crate) fn set_video_size(&self, width: i32, height: i32) {
        *self.size.lock().unwrap() = (width, height);
        // The first frame may have been skipped while the size was unknown.
        self.request_redraw(true);
    }

    fn video_size(&self) -> (i32, i32) {
        *self.size.lock().unwrap()
    }

    fn request_redraw(&self, force: bool) {
        if force {
            self.force_redraw.store(true, Ordering::Release);
        }
        *self.redraw.lock().unwrap() = true;
        self.redraw_cv.notify_one();
    }

    /// Blocks until a redraw is requested. Returns None once shutting down,
    /// otherwise whether the redraw was forced.
    fn wait_redraw(&self) -> Option<bool> {
        {
            let mut redraw = self.redraw.lock().unwrap();
            while !*redraw {
                redraw = self.redraw_cv.wait(redraw).unwrap();
            }
            *redraw = false;
        }
        if self.shutdown.load(Ordering::Acquire) {
            return None;
        }
        Some(self.force_redraw.swap(false, Ordering::AcqRel))
    }
}

/// Raw mpv handle for creating the render context. The caller guarantees mpv
/// outlives the [`VideoOutput`].
#[derive(Clone, Copy)]
pub(crate) struct MpvHandle(pub *mut mpv_handle);

unsafe impl Send for MpvHandle {}

/// What a renderer hands back once its thread is running.
struct Backend {
    texture_id: i64,
    thread: JoinHandle<()>,
    /// `hwdec` mode that fits the renderer.
    hwdec: &'static str,
}

pub(crate) struct VideoOutput {
    shared: Arc<VideoShared>,
    texture_id: i64,
    thread: Option<JoinHandle<()>>,
}

impl VideoOutput {
    /// Creates the Flutter texture and starts the render thread. Has to run
    /// before the first file is loaded, mpv can't show video without a render
    /// context. Returns the `hwdec` mode to set on mpv.
    pub(crate) fn start(mpv: MpvHandle, engine_handle: i64) -> Result<(Self, &'static str)> {
        let shared = VideoShared::new();
        let backend = start_backend(mpv, engine_handle, &shared)?;
        let output = Self {
            shared,
            texture_id: backend.texture_id,
            thread: Some(backend.thread),
        };
        Ok((output, backend.hwdec))
    }

    pub(crate) fn texture_id(&self) -> i64 {
        self.texture_id
    }

    pub(crate) fn shared(&self) -> Arc<VideoShared> {
        self.shared.clone()
    }

    /// Stops the render thread, which frees mpv's render context. Must happen
    /// before mpv is destroyed.
    pub(crate) fn stop(&mut self) {
        let Some(thread) = self.thread.take() else {
            return;
        };
        self.shared.shutdown.store(true, Ordering::Release);
        *self.shared.redraw.lock().unwrap() = true;
        self.shared.redraw_cv.notify_all();
        let _ = thread.join();
    }
}

impl Drop for VideoOutput {
    fn drop(&mut self) {
        self.stop();
    }
}

fn start_backend(mpv: MpvHandle, engine_handle: i64, shared: &Arc<VideoShared>) -> Result<Backend> {
    #[cfg(target_os = "linux")]
    if std::env::var("LPLAYER_RENDERER").as_deref() != Ok("sw") {
        match gl::start(mpv, engine_handle, shared.clone()) {
            Ok(backend) => return Ok(backend),
            Err(e) => eprintln!("lplayer: GPU rendering unavailable, using software: {e:#}"),
        }
    }
    sw::start(mpv, engine_handle, shared.clone())
}

/// Largest size that fits `video` into `max` without upscaling, None while
/// the video size is unknown.
fn fit_size(
    (width, height): (i32, i32),
    (max_width, max_height): (i32, i32),
) -> Option<(i32, i32)> {
    if width <= 0 || height <= 0 {
        return None;
    }
    let (w, h) = (width as f64, height as f64);
    let scale = (max_width as f64 / w).min(max_height as f64 / h).min(1.0);
    Some((
        ((w * scale).round() as i32).max(1),
        ((h * scale).round() as i32).max(1),
    ))
}

fn render_param<T>(type_: std::ffi::c_int, data: *mut T) -> mpv_render_param {
    mpv_render_param {
        type_,
        data: data as *mut c_void,
    }
}

const END_OF_PARAMS: mpv_render_param = mpv_render_param {
    type_: MPV_RENDER_PARAM_INVALID,
    data: ptr::null_mut(),
};

/// mpv render context, only used by the render thread after creation. Must be
/// dropped before mpv itself, and for OpenGL with the GL context current.
struct RenderContext(*mut mpv_render_context);

unsafe impl Send for RenderContext {}

impl RenderContext {
    /// `params` must end with [`END_OF_PARAMS`].
    unsafe fn create(mpv: MpvHandle, params: &mut [mpv_render_param]) -> Result<Self> {
        let mut ctx: *mut mpv_render_context = ptr::null_mut();
        check(
            mpv_render_context_create(&mut ctx, mpv.0, params.as_mut_ptr()),
            "mpv_render_context_create",
        )?;
        Ok(Self(ctx))
    }

    /// Routes mpv's update callback to `shared`, which has to outlive this context.
    fn set_update_callback(&self, shared: &Arc<VideoShared>) {
        unsafe {
            mpv_render_context_set_update_callback(
                self.0,
                Some(on_render_update),
                Arc::as_ptr(shared) as *mut c_void,
            )
        };
    }

    /// Has to be called on every wakeup, even if we end up not rendering.
    fn update(&self) -> u64 {
        unsafe { mpv_render_context_update(self.0) }
    }

    /// `params` must end with [`END_OF_PARAMS`].
    unsafe fn render(&self, params: &mut [mpv_render_param]) -> Result<()> {
        check(
            mpv_render_context_render(self.0, params.as_mut_ptr()),
            "mpv_render_context_render",
        )
    }
}

impl Drop for RenderContext {
    fn drop(&mut self) {
        unsafe {
            mpv_render_context_set_update_callback(self.0, None, ptr::null_mut());
            mpv_render_context_free(self.0);
        }
    }
}

unsafe extern "C" fn on_render_update(ctx: *mut c_void) {
    // Must not call into mpv here, just wake the render thread.
    let shared = &*(ctx as *const VideoShared);
    shared.request_redraw(false);
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
