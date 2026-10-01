//! GPU video output for Linux.
//!
//! mpv renders with OpenGL ES into textures of our own EGL context on the
//! render thread. Each texture is wrapped in an EGLImage, which Flutter's
//! raster context imports when it asks for the next frame.
//!
//! Flutter creates its EGL contexts on its own, so there is no context to
//! share with before the first frame is shown, and mpv needs its render
//! context before a file is loaded. EGLImages only need the same EGLDisplay,
//! which we get by asking EGL for the display of the same platform and native
//! display that Flutter uses (EGL returns the same handle for the same input).

use std::ffi::{c_int, c_void, CStr};
use std::ptr;
use std::sync::{mpsc, Arc, Mutex};

use anyhow::{anyhow, bail, Result};
use irondash_run_loop::RunLoop;
use irondash_texture::{
    BoxedGLTexture, GLTexture, GLTextureProvider, PayloadProvider, SendableTexture, Texture,
};

use super::egl::*;
use super::*;

/// Frames are rendered at the video's size up to 4K, Flutter scales them to the widget.
const MAX_FRAME_SIZE: (i32, i32) = (3840, 2160);

/// Textures rotating between mpv and Flutter: one Flutter shows, one waiting
/// to be shown and one mpv renders into.
const SLOT_COUNT: usize = 3;

/// mpv draws bottom-up like into a window, Flutter samples external textures top-down.
const FLIP_Y: c_int = 0;

pub(super) fn start(
    mpv: MpvHandle,
    engine_handle: i64,
    shared: Arc<VideoShared>,
) -> Result<Backend> {
    let egl = egl::get().map_err(|e| anyhow!(e))?;
    let main_thread = RunLoop::sender_for_main_thread()
        .map_err(|e| anyhow!("main thread is not available: {e:?}"))?;
    let native = main_thread
        .send_and_wait(|| unsafe { NativeDisplay::query() })
        .ok_or_else(|| anyhow!("GDK display is neither Wayland nor X11"))?;
    let display = unsafe { open_display(egl, native) }?;

    let frames = Arc::new(Frames::new(display));
    let provider = frames.clone();
    let (texture_id, texture) = main_thread
        .send_and_wait(move || {
            Texture::<BoxedGLTexture>::new_with_provider(engine_handle, provider)
                .map(|texture| (texture.id(), texture.into_sendable_texture()))
                .map_err(|e| e.to_string())
        })
        .map_err(|e| anyhow!("failed to create video texture: {e}"))?;
    let texture = MainThreadDrop::new(texture);

    // The GL context lives on the render thread, so mpv's render context has
    // to be created there too. Wait for it, mpv can't play video without one.
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let thread = std::thread::Builder::new()
        .name("lplayer-render".into())
        .spawn(move || {
            let renderer = match unsafe { Renderer::new(egl, display, native, mpv) } {
                Ok(renderer) => renderer,
                Err(e) => {
                    let _ = ready_tx.send(Err(e));
                    return;
                }
            };
            let _ = ready_tx.send(Ok(()));
            renderer.run(&shared, &frames, &texture);
        })?;

    match ready_rx.recv() {
        Ok(Ok(())) => Ok(Backend {
            texture_id,
            thread,
            hwdec: "auto-safe",
        }),
        Ok(Err(e)) => {
            let _ = thread.join();
            Err(e)
        }
        Err(_) => {
            let _ = thread.join();
            bail!("render thread exited during setup")
        }
    }
}

/// Native display of GDK, which Flutter uses for its EGL display too. Both
/// pointers are owned by GDK and stay valid for the app's lifetime.
#[derive(Clone, Copy)]
enum NativeDisplay {
    /// `struct wl_display*`
    Wayland(usize),
    /// Xlib `Display*`
    X11(usize),
}

impl NativeDisplay {
    /// Main thread only. GDK is looked up at runtime since the Flutter runner
    /// has it loaded anyway.
    unsafe fn query() -> Option<Self> {
        unsafe fn sym<T: Copy>(name: &CStr) -> Option<T> {
            let ptr = libc::dlsym(libc::RTLD_DEFAULT, name.as_ptr());
            (!ptr.is_null()).then(|| std::mem::transmute_copy(&ptr))
        }
        type GetDefault = unsafe extern "C" fn() -> *mut c_void;
        type GetType = unsafe extern "C" fn() -> usize;
        type GetNative = unsafe extern "C" fn(*mut c_void) -> *mut c_void;
        type IsA = unsafe extern "C" fn(*mut c_void, usize) -> c_int;

        let get_default: GetDefault = sym(c"gdk_display_get_default")?;
        let is_a: IsA = sym(c"g_type_check_instance_is_a")?;
        let display = get_default();
        if display.is_null() {
            return None;
        }

        let backends: [(&CStr, &CStr, fn(usize) -> Self); 2] = [
            (
                c"gdk_wayland_display_get_type",
                c"gdk_wayland_display_get_wl_display",
                Self::Wayland,
            ),
            (
                c"gdk_x11_display_get_type",
                c"gdk_x11_display_get_xdisplay",
                Self::X11,
            ),
        ];
        backends
            .into_iter()
            .find_map(|(get_type, get_native, wrap)| {
                let get_type: GetType = sym(get_type)?;
                let get_native: GetNative = sym(get_native)?;
                if is_a(display, get_type()) == 0 {
                    return None;
                }
                let native = get_native(display);
                (!native.is_null()).then(|| wrap(native as usize))
            })
    }

    fn ptr(self) -> *mut c_void {
        match self {
            Self::Wayland(ptr) | Self::X11(ptr) => ptr as *mut c_void,
        }
    }

    fn egl_platform(self) -> EGLenum {
        match self {
            Self::Wayland(_) => EGL_PLATFORM_WAYLAND,
            Self::X11(_) => EGL_PLATFORM_X11,
        }
    }

    /// mpv needs the native display for hardware decoding interop (VA-API).
    fn mpv_param(self) -> c_int {
        match self {
            Self::Wayland(_) => MPV_RENDER_PARAM_WL_DISPLAY,
            Self::X11(_) => MPV_RENDER_PARAM_X11_DISPLAY,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Display(EGLDisplay);

unsafe impl Send for Display {}
unsafe impl Sync for Display {}

unsafe fn open_display(egl: &Egl, native: NativeDisplay) -> Result<Display> {
    let get_platform_display = egl
        .get_platform_display
        .ok_or_else(|| anyhow!("EGL_EXT_platform_base is not supported"))?;
    let display = get_platform_display(native.egl_platform(), native.ptr(), ptr::null());
    if display.is_null() {
        return Err(egl.error("eglGetPlatformDisplayEXT"));
    }
    // Flutter initialized it already, doing it again is a no-op. Never
    // terminate it, that would pull the display from under Flutter.
    if (egl.initialize)(display, ptr::null_mut(), ptr::null_mut()) != EGL_TRUE {
        return Err(egl.error("eglInitialize"));
    }
    for extension in [
        "EGL_KHR_image_base",
        "EGL_KHR_gl_texture_2D_image",
        "EGL_KHR_surfaceless_context",
    ] {
        if !egl.has_extension(display, extension) {
            bail!("{extension} is not supported");
        }
    }
    Ok(Display(display))
}

// ---------------------------------------------------------------------------
// Render thread
// ---------------------------------------------------------------------------

/// Owns our EGL context and mpv's render context. Lives on the render thread
/// with the context current the whole time.
struct Renderer {
    egl: &'static Egl,
    display: Display,
    context: EGLContext,
    render_ctx: Option<RenderContext>,
    slots: [RenderSlot; SLOT_COUNT],
    last_error: Option<String>,
}

impl Renderer {
    unsafe fn new(
        egl: &'static Egl,
        display: Display,
        native: NativeDisplay,
        mpv: MpvHandle,
    ) -> Result<Self> {
        // Per thread state, so this has to happen on the render thread.
        if (egl.bind_api)(EGL_OPENGL_ES_API) != EGL_TRUE {
            return Err(egl.error("eglBindAPI"));
        }
        let context = create_context(egl, display)?;
        // From here on Drop cleans up.
        let mut renderer = Self {
            egl,
            display,
            context,
            render_ctx: None,
            slots: Default::default(),
            last_error: None,
        };
        // Surfaceless: mpv only ever draws into our framebuffers.
        if (egl.make_current)(display.0, EGL_NO_SURFACE, EGL_NO_SURFACE, context) != EGL_TRUE {
            return Err(egl.error("eglMakeCurrent"));
        }

        let mut init = mpv_opengl_init_params {
            get_proc_address: Some(get_proc_address),
            get_proc_address_ctx: ptr::null_mut(),
        };
        let mut params = [
            render_param(
                MPV_RENDER_PARAM_API_TYPE,
                MPV_RENDER_API_TYPE_OPENGL.as_ptr() as *mut u8,
            ),
            render_param(MPV_RENDER_PARAM_OPENGL_INIT_PARAMS, &mut init),
            render_param(native.mpv_param(), native.ptr()),
            END_OF_PARAMS,
        ];
        renderer.render_ctx = Some(RenderContext::create(mpv, &mut params)?);
        Ok(renderer)
    }

    fn run(
        mut self,
        shared: &Arc<VideoShared>,
        frames: &Frames,
        texture: &Arc<SendableTexture<BoxedGLTexture>>,
    ) {
        if let Some(ctx) = &self.render_ctx {
            ctx.set_update_callback(shared);
        }
        while let Some(forced) = shared.wait_redraw() {
            let Some(ctx) = &self.render_ctx else { break };
            let flags = ctx.update();
            if flags & MPV_RENDER_UPDATE_FRAME == 0 && !forced {
                continue;
            }
            let Some(size) = fit_size(shared.video_size(), MAX_FRAME_SIZE) else {
                continue;
            };
            match unsafe { self.render_frame(size, frames) } {
                Ok(()) => {
                    self.last_error = None;
                    texture.mark_frame_available();
                }
                Err(e) => {
                    // Usually repeats every frame, only log when it changes.
                    let message = format!("{e:#}");
                    if self.last_error.as_ref() != Some(&message) {
                        eprintln!("lplayer: rendering failed: {message}");
                        self.last_error = Some(message);
                    }
                }
            }
        }
        // Flutter must stop importing our images before they are destroyed below.
        frames.close();
        // Dropping frees the render context while `shared`, the update
        // callback's target, is still alive.
    }

    unsafe fn render_frame(&mut self, (width, height): (i32, i32), frames: &Frames) -> Result<()> {
        let ctx = self.render_ctx.as_ref().expect("checked by run");
        let index = frames.free_slot();
        let slot = &mut self.slots[index];
        if (slot.width, slot.height) != (width, height) {
            slot.release(self.egl, self.display);
            slot.allocate(self.egl, self.display, self.context, width, height)?;
            frames.replace_image(index, slot.image, width, height);
        }

        let mut fbo = mpv_opengl_fbo {
            fbo: slot.fbo as c_int,
            w: width,
            h: height,
            internal_format: GL_RGBA8 as c_int,
        };
        let mut flip_y = FLIP_Y;
        let mut params = [
            render_param(MPV_RENDER_PARAM_OPENGL_FBO, &mut fbo),
            render_param(MPV_RENDER_PARAM_FLIP_Y, &mut flip_y),
            END_OF_PARAMS,
        ];
        ctx.render(&mut params)?;
        // Flutter reads the texture from another context, so the frame has to
        // be done on the GPU first. Only blocks this thread.
        (self.egl.gl.finish)();
        frames.publish(index);
        Ok(())
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        unsafe {
            // mpv frees its GL objects here, the context is still current.
            self.render_ctx.take();
            for slot in &mut self.slots {
                slot.release(self.egl, self.display);
            }
            let egl = self.egl;
            (egl.make_current)(
                self.display.0,
                EGL_NO_SURFACE,
                EGL_NO_SURFACE,
                EGL_NO_CONTEXT,
            );
            (egl.destroy_context)(self.display.0, self.context);
        }
    }
}

/// GLES 3 if available, mpv also copes with GLES 2.
unsafe fn create_context(egl: &Egl, display: Display) -> Result<EGLContext> {
    for (version, renderable) in [(3, EGL_OPENGL_ES3_BIT), (2, EGL_OPENGL_ES2_BIT)] {
        let config_attribs = [
            EGL_RED_SIZE,
            8,
            EGL_GREEN_SIZE,
            8,
            EGL_BLUE_SIZE,
            8,
            EGL_ALPHA_SIZE,
            8,
            EGL_RENDERABLE_TYPE,
            renderable,
            EGL_NONE,
        ];
        let mut config: EGLConfig = ptr::null_mut();
        let mut count = 0;
        if (egl.choose_config)(
            display.0,
            config_attribs.as_ptr(),
            &mut config,
            1,
            &mut count,
        ) != EGL_TRUE
            || count < 1
        {
            continue;
        }
        let context_attribs = [EGL_CONTEXT_CLIENT_VERSION, version, EGL_NONE];
        let context =
            (egl.create_context)(display.0, config, EGL_NO_CONTEXT, context_attribs.as_ptr());
        if !context.is_null() {
            return Ok(context);
        }
    }
    Err(egl.error("eglCreateContext"))
}

unsafe extern "C" fn get_proc_address(
    _ctx: *mut c_void,
    name: *const std::ffi::c_char,
) -> *mut c_void {
    match egl::get() {
        Ok(egl) => (egl.get_proc_address)(name),
        Err(_) => ptr::null_mut(),
    }
}

/// A texture mpv renders into, plus the framebuffer and EGLImage around it.
struct RenderSlot {
    texture: GLuint,
    fbo: GLuint,
    image: EGLImage,
    width: i32,
    height: i32,
}

impl Default for RenderSlot {
    fn default() -> Self {
        Self {
            texture: 0,
            fbo: 0,
            image: ptr::null_mut(),
            width: 0,
            height: 0,
        }
    }
}

impl RenderSlot {
    unsafe fn allocate(
        &mut self,
        egl: &Egl,
        display: Display,
        context: EGLContext,
        width: i32,
        height: i32,
    ) -> Result<()> {
        let gl = &egl.gl;
        (gl.gen_textures)(1, &mut self.texture);
        (gl.bind_texture)(GL_TEXTURE_2D, self.texture);
        set_texture_params(gl);
        (gl.tex_image_2d)(
            GL_TEXTURE_2D,
            0,
            GL_RGBA as GLint,
            width,
            height,
            0,
            GL_RGBA,
            GL_UNSIGNED_BYTE,
            ptr::null(),
        );
        (gl.bind_texture)(GL_TEXTURE_2D, 0);

        let image_attribs = [EGL_GL_TEXTURE_LEVEL, 0, EGL_NONE];
        self.image = (egl.create_image)(
            display.0,
            context,
            EGL_GL_TEXTURE_2D,
            self.texture as usize as EGLClientBuffer,
            image_attribs.as_ptr(),
        );
        if self.image.is_null() {
            return Err(egl.error("eglCreateImageKHR"));
        }

        (gl.gen_framebuffers)(1, &mut self.fbo);
        (gl.bind_framebuffer)(GL_FRAMEBUFFER, self.fbo);
        (gl.framebuffer_texture_2d)(
            GL_FRAMEBUFFER,
            GL_COLOR_ATTACHMENT0,
            GL_TEXTURE_2D,
            self.texture,
            0,
        );
        let status = (gl.check_framebuffer_status)(GL_FRAMEBUFFER);
        (gl.bind_framebuffer)(GL_FRAMEBUFFER, 0);
        if status != GL_FRAMEBUFFER_COMPLETE {
            bail!("video framebuffer is incomplete ({status:#x})");
        }

        self.width = width;
        self.height = height;
        Ok(())
    }

    /// Flutter's imported copy keeps the image's memory alive until it lets go.
    unsafe fn release(&mut self, egl: &Egl, display: Display) {
        if !self.image.is_null() {
            (egl.destroy_image)(display.0, self.image);
        }
        if self.fbo != 0 {
            (egl.gl.delete_framebuffers)(1, &self.fbo);
        }
        if self.texture != 0 {
            (egl.gl.delete_textures)(1, &self.texture);
        }
        *self = Self::default();
    }
}

unsafe fn set_texture_params(gl: &Gl) {
    for (name, value) in [
        (GL_TEXTURE_MIN_FILTER, GL_LINEAR),
        (GL_TEXTURE_MAG_FILTER, GL_LINEAR),
        (GL_TEXTURE_WRAP_S, GL_CLAMP_TO_EDGE),
        (GL_TEXTURE_WRAP_T, GL_CLAMP_TO_EDGE),
    ] {
        (gl.tex_parameteri)(GL_TEXTURE_2D, name, value);
    }
}

// ---------------------------------------------------------------------------
// Frame hand-off to Flutter
// ---------------------------------------------------------------------------

/// Shared between the render thread and Flutter's raster thread.
struct Frames {
    display: Display,
    state: Mutex<FramesState>,
}

struct FramesState {
    slots: [SharedSlot; SLOT_COUNT],
    /// Newest finished frame, the one Flutter shows next.
    published: Option<usize>,
    /// Frame Flutter samples right now, until it asks for a new one.
    displayed: Option<usize>,
    /// Set when the render thread is about to destroy its images.
    closed: bool,
    display_mismatch_logged: bool,
}

unsafe impl Send for FramesState {}

struct SharedSlot {
    image: EGLImage,
    /// Bumped whenever `image` is replaced.
    generation: u64,
    width: i32,
    height: i32,
    /// Texture in Flutter's context that `image` was imported into.
    flutter_texture: GLuint,
    flutter_generation: u64,
}

impl Default for SharedSlot {
    fn default() -> Self {
        Self {
            image: ptr::null_mut(),
            generation: 0,
            width: 0,
            height: 0,
            flutter_texture: 0,
            flutter_generation: 0,
        }
    }
}

impl Frames {
    fn new(display: Display) -> Self {
        Self {
            display,
            state: Mutex::new(FramesState {
                slots: Default::default(),
                published: None,
                displayed: None,
                closed: false,
                display_mismatch_logged: false,
            }),
        }
    }

    /// A slot Flutter neither shows nor is about to show.
    fn free_slot(&self) -> usize {
        let state = self.state.lock().unwrap();
        (0..SLOT_COUNT)
            .find(|index| state.published != Some(*index) && state.displayed != Some(*index))
            .expect("at most two slots are in use")
    }

    fn replace_image(&self, index: usize, image: EGLImage, width: i32, height: i32) {
        let mut state = self.state.lock().unwrap();
        let slot = &mut state.slots[index];
        slot.image = image;
        slot.generation += 1;
        slot.width = width;
        slot.height = height;
    }

    fn publish(&self, index: usize) {
        self.state.lock().unwrap().published = Some(index);
    }

    fn close(&self) {
        self.state.lock().unwrap().closed = true;
    }
}

impl PayloadProvider<BoxedGLTexture> for Frames {
    /// Called on Flutter's raster thread with Flutter's GL context current.
    fn get_payload(&self) -> BoxedGLTexture {
        let mut state = self.state.lock().unwrap();
        Box::new(unsafe { state.acquire(self.display) }.unwrap_or(GlFrame::EMPTY))
    }
}

impl FramesState {
    unsafe fn acquire(&mut self, display: Display) -> Option<GlFrame> {
        let index = self.published?;
        let stale = self.slots[index].flutter_generation != self.slots[index].generation;
        if stale {
            if self.closed {
                return None;
            }
            let egl = egl::get().ok()?;
            if (egl.get_current_display)() != display.0 {
                if !self.display_mismatch_logged {
                    eprintln!("lplayer: Flutter uses a different EGL display, video can't be shown. Run with LPLAYER_RENDERER=sw");
                    self.display_mismatch_logged = true;
                }
                return None;
            }
            import(egl, &mut self.slots[index]);
        }
        self.displayed = Some(index);
        let slot = &self.slots[index];
        Some(GlFrame {
            name: slot.flutter_texture,
            width: slot.width,
            height: slot.height,
        })
    }
}

/// Points the slot's Flutter side texture at its current image. Never runs
/// for the slot Flutter is showing, only for the one it is about to show.
///
/// The Flutter side textures are never deleted: that needs Flutter's context,
/// which we only get here. The player lives as long as the app, so at most
/// [`SLOT_COUNT`] textures are left behind.
unsafe fn import(egl: &Egl, slot: &mut SharedSlot) {
    let gl = &egl.gl;
    let mut previous: GLint = 0;
    (gl.get_integerv)(GL_TEXTURE_BINDING_2D, &mut previous);
    if slot.flutter_texture == 0 {
        (gl.gen_textures)(1, &mut slot.flutter_texture);
    }
    (gl.bind_texture)(GL_TEXTURE_2D, slot.flutter_texture);
    (gl.egl_image_target_texture_2d)(GL_TEXTURE_2D, slot.image);
    set_texture_params(gl);
    // Flutter's renderer caches GL state, leave it as we found it.
    (gl.bind_texture)(GL_TEXTURE_2D, previous as GLuint);
    slot.flutter_generation = slot.generation;
}

struct GlFrame {
    name: GLuint,
    width: i32,
    height: i32,
}

impl GlFrame {
    /// Nothing to show yet. Flutter skips drawing and asks again next frame.
    const EMPTY: Self = Self {
        name: 0,
        width: 1,
        height: 1,
    };
}

impl GLTextureProvider for GlFrame {
    fn get(&self) -> GLTexture<'_> {
        GLTexture {
            target: GL_TEXTURE_2D,
            name: &self.name,
            width: self.width,
            height: self.height,
        }
    }
}
