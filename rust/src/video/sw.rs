//! Software video output: mpv renders into RGBA buffers on the CPU, which are
//! shown through a Flutter pixel buffer texture.

use std::ffi::{c_int, CStr};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use irondash_run_loop::RunLoop;
use irondash_texture::{
    BoxedPixelData, PayloadProvider, PixelData, PixelDataProvider, PixelFormat, SendableTexture,
    SimplePixelData, Texture,
};

use super::*;

/// Frames bigger than this are downscaled by mpv to keep the CPU copy cheap.
const MAX_FRAME_SIZE: (i32, i32) = (1920, 1080);

/// How many spare frame buffers the render thread keeps for reuse.
const FRAME_POOL_SIZE: usize = 3;

pub(super) fn start(
    mpv: MpvHandle,
    engine_handle: i64,
    shared: Arc<VideoShared>,
) -> Result<Backend> {
    let mut params = [
        render_param(
            MPV_RENDER_PARAM_API_TYPE,
            MPV_RENDER_API_TYPE_SW.as_ptr() as *mut u8,
        ),
        END_OF_PARAMS,
    ];
    let ctx = unsafe { RenderContext::create(mpv, &mut params) }?;

    // Flutter textures have to be created on the platform (main) thread.
    let main_thread = RunLoop::sender_for_main_thread()
        .map_err(|e| anyhow!("main thread is not available: {e:?}"))?;
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

    let thread = std::thread::Builder::new()
        .name("lplayer-render".into())
        .spawn(move || render_loop(shared, ctx, frames, texture))?;
    Ok(Backend {
        texture_id,
        thread,
        hwdec: "auto-copy",
    })
}

fn render_loop(
    shared: Arc<VideoShared>,
    ctx: RenderContext,
    frames: Arc<FrameProvider>,
    texture: MainThreadDrop<Arc<SendableTexture<BoxedPixelData>>>,
) {
    let pixel_format: &CStr = match PixelData::FORMAT {
        PixelFormat::RGBA => c"rgb0",
        PixelFormat::BGRA => c"bgr0",
    };
    ctx.set_update_callback(&shared);

    let mut pool: Vec<Arc<Vec<u8>>> = Vec::new();
    while let Some(forced) = shared.wait_redraw() {
        let flags = ctx.update();
        if flags & MPV_RENDER_UPDATE_FRAME == 0 && !forced {
            continue;
        }
        let Some((width, height)) = frame_size(shared.video_size()) else {
            continue;
        };

        let mut buffer = take_buffer(&mut pool, (width * height * 4) as usize);
        let pixels = Arc::get_mut(&mut buffer).expect("buffer from pool is unique");

        let mut sw_size: [c_int; 2] = [width, height];
        let mut stride: usize = width as usize * 4;
        let mut params = [
            render_param(MPV_RENDER_PARAM_SW_SIZE, sw_size.as_mut_ptr()),
            render_param(MPV_RENDER_PARAM_SW_FORMAT, pixel_format.as_ptr().cast_mut()),
            render_param(MPV_RENDER_PARAM_SW_STRIDE, &mut stride),
            render_param(MPV_RENDER_PARAM_SW_POINTER, pixels.as_mut_ptr()),
            END_OF_PARAMS,
        ];
        if unsafe { ctx.render(&mut params) }.is_err() {
            pool.push(buffer);
            continue;
        }

        // "rgb0" leaves the 4th byte undefined, Flutter needs it opaque.
        for pixel in pixels.chunks_exact_mut(4) {
            pixel[3] = 255;
        }

        if let Some(previous) = frames.publish(Frame {
            width,
            height,
            data: buffer,
        }) {
            if pool.len() < FRAME_POOL_SIZE {
                pool.push(previous);
            }
        }
        texture.mark_frame_available();
    }

    // Free the render context while `shared`, the update callback's target, is still alive.
    drop(ctx);
}

/// Width is kept a multiple of 16 so rows stay 64 byte aligned for mpv.
fn frame_size(video_size: (i32, i32)) -> Option<(i32, i32)> {
    let (width, height) = fit_size(video_size, MAX_FRAME_SIZE)?;
    Some(((width / 16 * 16).max(16), height.max(2)))
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
