//! EGL and the few OpenGL ES functions the GPU renderer needs, loaded at
//! runtime. Using dlopen keeps EGL out of the build requirements and lets the
//! player fall back to software rendering when it is missing.

#![allow(non_camel_case_types, dead_code)]

use std::ffi::{c_char, c_uint, c_void, CStr};
use std::sync::OnceLock;

pub type EGLDisplay = *mut c_void;
pub type EGLContext = *mut c_void;
pub type EGLSurface = *mut c_void;
pub type EGLConfig = *mut c_void;
pub type EGLImage = *mut c_void;
pub type EGLClientBuffer = *mut c_void;
pub type EGLBoolean = c_uint;
pub type EGLenum = c_uint;
pub type EGLint = i32;

pub type GLenum = u32;
pub type GLuint = u32;
pub type GLint = i32;
pub type GLsizei = i32;

pub const EGL_TRUE: EGLBoolean = 1;
pub const EGL_NONE: EGLint = 0x3038;
pub const EGL_ALPHA_SIZE: EGLint = 0x3021;
pub const EGL_BLUE_SIZE: EGLint = 0x3022;
pub const EGL_GREEN_SIZE: EGLint = 0x3023;
pub const EGL_RED_SIZE: EGLint = 0x3024;
pub const EGL_RENDERABLE_TYPE: EGLint = 0x3040;
pub const EGL_OPENGL_ES2_BIT: EGLint = 0x0004;
pub const EGL_OPENGL_ES3_BIT: EGLint = 0x0040;
pub const EGL_EXTENSIONS: EGLint = 0x3055;
pub const EGL_CONTEXT_CLIENT_VERSION: EGLint = 0x3098;
pub const EGL_OPENGL_ES_API: EGLenum = 0x30A0;
pub const EGL_GL_TEXTURE_2D: EGLenum = 0x30B1;
pub const EGL_GL_TEXTURE_LEVEL: EGLint = 0x30BC;
pub const EGL_PLATFORM_X11: EGLenum = 0x31D5;
pub const EGL_PLATFORM_WAYLAND: EGLenum = 0x31D8;

pub const EGL_NO_CONTEXT: EGLContext = std::ptr::null_mut();
pub const EGL_NO_SURFACE: EGLSurface = std::ptr::null_mut();

pub const GL_TEXTURE_2D: GLenum = 0x0DE1;
pub const GL_TEXTURE_BINDING_2D: GLenum = 0x8069;
pub const GL_TEXTURE_MAG_FILTER: GLenum = 0x2800;
pub const GL_TEXTURE_MIN_FILTER: GLenum = 0x2801;
pub const GL_TEXTURE_WRAP_S: GLenum = 0x2802;
pub const GL_TEXTURE_WRAP_T: GLenum = 0x2803;
pub const GL_LINEAR: GLint = 0x2601;
pub const GL_CLAMP_TO_EDGE: GLint = 0x812F;
pub const GL_RGBA: GLenum = 0x1908;
pub const GL_RGBA8: GLenum = 0x8058;
pub const GL_UNSIGNED_BYTE: GLenum = 0x1401;
pub const GL_FRAMEBUFFER: GLenum = 0x8D40;
pub const GL_COLOR_ATTACHMENT0: GLenum = 0x8CE0;
pub const GL_FRAMEBUFFER_COMPLETE: GLenum = 0x8CD5;

pub struct Egl {
    pub get_proc_address: unsafe extern "C" fn(*const c_char) -> *mut c_void,
    /// From EGL_EXT_platform_base, None if the extension is missing.
    pub get_platform_display:
        Option<unsafe extern "C" fn(EGLenum, *mut c_void, *const EGLint) -> EGLDisplay>,
    pub initialize: unsafe extern "C" fn(EGLDisplay, *mut EGLint, *mut EGLint) -> EGLBoolean,
    pub query_string: unsafe extern "C" fn(EGLDisplay, EGLint) -> *const c_char,
    pub get_error: unsafe extern "C" fn() -> EGLint,
    pub bind_api: unsafe extern "C" fn(EGLenum) -> EGLBoolean,
    pub choose_config: unsafe extern "C" fn(
        EGLDisplay,
        *const EGLint,
        *mut EGLConfig,
        EGLint,
        *mut EGLint,
    ) -> EGLBoolean,
    pub create_context:
        unsafe extern "C" fn(EGLDisplay, EGLConfig, EGLContext, *const EGLint) -> EGLContext,
    pub destroy_context: unsafe extern "C" fn(EGLDisplay, EGLContext) -> EGLBoolean,
    pub make_current:
        unsafe extern "C" fn(EGLDisplay, EGLSurface, EGLSurface, EGLContext) -> EGLBoolean,
    pub get_current_display: unsafe extern "C" fn() -> EGLDisplay,
    /// eglCreateImageKHR / eglDestroyImageKHR (EGL_KHR_image_base).
    pub create_image: unsafe extern "C" fn(
        EGLDisplay,
        EGLContext,
        EGLenum,
        EGLClientBuffer,
        *const EGLint,
    ) -> EGLImage,
    pub destroy_image: unsafe extern "C" fn(EGLDisplay, EGLImage) -> EGLBoolean,
    pub gl: Gl,
}

/// OpenGL ES functions. They dispatch to whichever context is current, so the
/// same pointers work for our context and Flutter's.
pub struct Gl {
    pub gen_textures: unsafe extern "C" fn(GLsizei, *mut GLuint),
    pub delete_textures: unsafe extern "C" fn(GLsizei, *const GLuint),
    pub bind_texture: unsafe extern "C" fn(GLenum, GLuint),
    pub tex_parameteri: unsafe extern "C" fn(GLenum, GLenum, GLint),
    pub tex_image_2d: unsafe extern "C" fn(
        GLenum,
        GLint,
        GLint,
        GLsizei,
        GLsizei,
        GLint,
        GLenum,
        GLenum,
        *const c_void,
    ),
    pub gen_framebuffers: unsafe extern "C" fn(GLsizei, *mut GLuint),
    pub delete_framebuffers: unsafe extern "C" fn(GLsizei, *const GLuint),
    pub bind_framebuffer: unsafe extern "C" fn(GLenum, GLuint),
    pub framebuffer_texture_2d: unsafe extern "C" fn(GLenum, GLenum, GLenum, GLuint, GLint),
    pub check_framebuffer_status: unsafe extern "C" fn(GLenum) -> GLenum,
    pub get_integerv: unsafe extern "C" fn(GLenum, *mut GLint),
    pub finish: unsafe extern "C" fn(),
    /// glEGLImageTargetTexture2DOES (GL_OES_EGL_image).
    pub egl_image_target_texture_2d: unsafe extern "C" fn(GLenum, EGLImage),
}

/// Loads EGL once per process.
pub fn get() -> Result<&'static Egl, String> {
    static EGL: OnceLock<Result<Egl, String>> = OnceLock::new();
    EGL.get_or_init(|| unsafe { Egl::load() })
        .as_ref()
        .map_err(Clone::clone)
}

impl Egl {
    unsafe fn load() -> Result<Self, String> {
        let lib = libc::dlopen(c"libEGL.so.1".as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL);
        if lib.is_null() {
            return Err("libEGL.so.1 not found".into());
        }
        let dl = |name: &CStr| libc::dlsym(lib, name.as_ptr());
        let get_proc_address: unsafe extern "C" fn(*const c_char) -> *mut c_void =
            required(dl(c"eglGetProcAddress"), c"eglGetProcAddress")?;
        let proc = |name: &CStr| get_proc_address(name.as_ptr());

        macro_rules! egl {
            ($name:literal) => {
                required(dl($name), $name)?
            };
        }
        macro_rules! ext {
            ($name:literal) => {
                required(proc($name), $name)?
            };
        }

        Ok(Self {
            get_proc_address,
            get_platform_display: optional(proc(c"eglGetPlatformDisplayEXT")),
            initialize: egl!(c"eglInitialize"),
            query_string: egl!(c"eglQueryString"),
            get_error: egl!(c"eglGetError"),
            bind_api: egl!(c"eglBindAPI"),
            choose_config: egl!(c"eglChooseConfig"),
            create_context: egl!(c"eglCreateContext"),
            destroy_context: egl!(c"eglDestroyContext"),
            make_current: egl!(c"eglMakeCurrent"),
            get_current_display: egl!(c"eglGetCurrentDisplay"),
            create_image: ext!(c"eglCreateImageKHR"),
            destroy_image: ext!(c"eglDestroyImageKHR"),
            gl: Gl {
                gen_textures: ext!(c"glGenTextures"),
                delete_textures: ext!(c"glDeleteTextures"),
                bind_texture: ext!(c"glBindTexture"),
                tex_parameteri: ext!(c"glTexParameteri"),
                tex_image_2d: ext!(c"glTexImage2D"),
                gen_framebuffers: ext!(c"glGenFramebuffers"),
                delete_framebuffers: ext!(c"glDeleteFramebuffers"),
                bind_framebuffer: ext!(c"glBindFramebuffer"),
                framebuffer_texture_2d: ext!(c"glFramebufferTexture2D"),
                check_framebuffer_status: ext!(c"glCheckFramebufferStatus"),
                get_integerv: ext!(c"glGetIntegerv"),
                finish: ext!(c"glFinish"),
                egl_image_target_texture_2d: ext!(c"glEGLImageTargetTexture2DOES"),
            },
        })
    }

    pub unsafe fn has_extension(&self, display: EGLDisplay, name: &str) -> bool {
        let list = (self.query_string)(display, EGL_EXTENSIONS);
        !list.is_null()
            && CStr::from_ptr(list)
                .to_string_lossy()
                .split_ascii_whitespace()
                .any(|extension| extension == name)
    }

    /// Error for a failed EGL call, with the code from eglGetError.
    pub fn error(&self, what: &str) -> anyhow::Error {
        let code = unsafe { (self.get_error)() };
        anyhow::anyhow!("{what} failed (EGL error {code:#x})")
    }
}

/// `T` must be a function pointer type.
unsafe fn required<T: Copy>(ptr: *mut c_void, name: &CStr) -> Result<T, String> {
    optional(ptr).ok_or_else(|| format!("{} is missing", name.to_string_lossy()))
}

unsafe fn optional<T: Copy>(ptr: *mut c_void) -> Option<T> {
    debug_assert_eq!(std::mem::size_of::<T>(), std::mem::size_of::<*mut c_void>());
    (!ptr.is_null()).then(|| std::mem::transmute_copy(&ptr))
}
