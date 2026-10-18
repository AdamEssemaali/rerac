//! The window: winit on a `CAMetalLayer`, the events fed to the input state, one engine frame and one render per redraw,
//! and the window state the systems ask for (fullscreen, cursor, vsync) applied after each frame.

use crate::determinism::Captures;
use crate::engine::app::{App, Window as WindowState};
use crate::engine::input::InputState;
use crate::render::frame::Renderer;
use anyhow::{Context, Result};
use objc2::rc::{autoreleasepool, Retained};
use objc2::runtime::ProtocolObject;
use objc2_core_foundation::CGSize;
use objc2_metal::{MTLCommandBuffer, MTLDrawable, MTLTexture};
use objc2_quartz_core::{CAMetalDrawable, CAMetalLayer};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::PhysicalKey;
use winit::window::{CursorGrabMode, Fullscreen, Window, WindowId};

pub fn run(app: App, renderer: Renderer) -> Result<i32> {
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut h = Handler { app, renderer, win: None, error: None, status: 0 };
    event_loop.run_app(&mut h)?;
    if let Some(e) = h.error { return Err(e); }
    Ok(h.status)
}

struct Win {
    window: Window,
    layer: Retained<CAMetalLayer>,
    /// The state last applied to winit.
    applied: WindowState,
}

struct Handler {
    app: App,
    renderer: Renderer,
    win: Option<Win>,
    error: Option<anyhow::Error>,
    status: i32,
}

impl Handler {
    fn open(&mut self, event_loop: &ActiveEventLoop) -> Result<()> {
        let title = self.app.ctx.res::<WindowState>().title.clone();
        let window = event_loop.create_window(Window::default_attributes().with_title(title).with_inner_size(LogicalSize::new(1024.0, 768.0)))?;
        let RawWindowHandle::AppKit(h) = window.window_handle()?.as_raw() else { anyhow::bail!("not an AppKit window") };
        // SAFETY: the NSView belongs to `window`, which lives as long as the layer (both in `Win`).
        let layer = unsafe { raw_window_metal::Layer::from_ns_view(h.ns_view) };
        // SAFETY: raw-window-metal hands out a retained CAMetalLayer.
        let layer: Retained<CAMetalLayer> = unsafe { Retained::from_raw(layer.into_raw().as_ptr().cast()) }.context("CAMetalLayer")?;
        layer.setDevice(Some(&self.renderer.gfx.device));
        layer.setPixelFormat(crate::render::gfx::pixel_format(crate::engine::image::TextureFormat::Bgra8UnormSrgb));
        // Readable, for `RC_CAPTURE_WINDOW=1`.
        layer.setFramebufferOnly(false);
        layer.setMaximumDrawableCount(3);
        let applied = self.app.ctx.res::<WindowState>().clone();
        self.win = Some(Win { window, layer, applied });
        self.sync_size();
        Ok(())
    }

    /// The drawable and the window resource follow the window's size and scale.
    fn sync_size(&mut self) {
        let Some(w) = self.win.as_ref() else { return };
        let size = w.window.inner_size();
        let scale = w.window.scale_factor();
        w.layer.setContentsScale(scale);
        w.layer.setDrawableSize(CGSize { width: size.width.max(1) as f64, height: size.height.max(1) as f64 });
        let refresh = w.window.current_monitor().and_then(|m| m.refresh_rate_millihertz());
        let mut ws = self.app.ctx.res_mut::<WindowState>();
        ws.physical_size = glam::UVec2::new(size.width.max(1), size.height.max(1));
        ws.scale_factor = scale;
        ws.refresh_millihertz = refresh;
    }

    /// What the systems changed in the window resource, applied to the window.
    fn apply_window(&mut self) {
        let Some(w) = self.win.as_mut() else { return };
        let want = self.app.ctx.res::<WindowState>().clone();
        if want.fullscreen != w.applied.fullscreen {
            w.window.set_fullscreen(want.fullscreen.then(|| Fullscreen::Borderless(None)));
        }
        if want.cursor_grabbed != w.applied.cursor_grabbed {
            let mode = if want.cursor_grabbed { CursorGrabMode::Locked } else { CursorGrabMode::None };
            if w.window.set_cursor_grab(mode).is_err() && want.cursor_grabbed { let _ = w.window.set_cursor_grab(CursorGrabMode::Confined); }
        }
        if want.cursor_visible != w.applied.cursor_visible { w.window.set_cursor_visible(want.cursor_visible); }
        if want.vsync != w.applied.vsync { w.layer.setDisplaySyncEnabled(want.vsync); }
        if want.title != w.applied.title { w.window.set_title(&want.title); }
        w.applied = want;
    }

    fn frame(&mut self) -> Result<()> {
        let t0 = std::time::Instant::now();
        self.sync_size();
        self.app.update();
        let t_update = t0.elapsed();
        self.app.end_frame_input();
        self.apply_window();
        let Some(w) = self.win.as_ref() else { return Ok(()) };
        let layer = w.layer.clone();
        let captures = std::mem::take(&mut self.app.ctx.res_mut::<Captures>().0);
        autoreleasepool(|_| -> Result<()> {
            let drawable = layer.nextDrawable();
            let texture = drawable.as_ref().map(|d| d.texture());
            let present: Option<&ProtocolObject<dyn MTLDrawable>> = drawable.as_deref().map(ProtocolObject::from_ref);
            let t1 = std::time::Instant::now();
            let cb = self.renderer.render(&mut self.app.ctx, texture.as_deref(), present)?;
            profile(t_update, t1.elapsed(), self.renderer.last_wait);
            if !captures.is_empty() {
                cb.waitUntilCompleted();
                for c in captures {
                    let px = match &c.target {
                        Some(h) => self.renderer.read_image(h.id())?,
                        None => match texture.as_deref() {
                            Some(t) => read_texture(&mut self.renderer, t)?,
                            None => continue,
                        },
                    };
                    (c.done)(px.0, px.1, &px.2, &mut self.app.ctx.res);
                }
            }
            Ok(())
        })?;
        Ok(())
    }
}

/// `RC_METAL_PROFILE=1`: the mean update, render (encode, after the wait) and wait times over 5 s.
fn profile(update: std::time::Duration, render: std::time::Duration, wait: std::time::Duration) {
    use std::sync::Mutex;
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ON.get_or_init(|| std::env::var("RC_METAL_PROFILE").is_ok_and(|v| v.trim() == "1")) { return; }
    static ACC: Mutex<Option<(std::time::Instant, u32, f64, f64, f64)>> = Mutex::new(None);
    let mut a = ACC.lock().unwrap();
    let e = a.get_or_insert((std::time::Instant::now(), 0, 0.0, 0.0, 0.0));
    e.1 += 1;
    e.2 += update.as_secs_f64();
    e.3 += (render - wait.min(render)).as_secs_f64();
    e.4 += wait.as_secs_f64();
    if e.0.elapsed().as_secs_f32() >= 5.0 {
        let n = e.1 as f64;
        println!("profile: {} frames: update {:.2} ms, encode {:.2} ms, wait {:.2} ms", e.1, e.2 / n * 1e3, e.3 / n * 1e3, e.4 / n * 1e3);
        *e = (std::time::Instant::now(), 0, 0.0, 0.0, 0.0);
    }
}

/// The drawable's pixels (BGRA) as RGBA.
fn read_texture(r: &mut Renderer, t: &ProtocolObject<dyn MTLTexture>) -> Result<(u32, u32, Vec<u8>)> {
    use objc2_metal::*;
    let (w, h) = (t.width(), t.height());
    let buf = r.gfx.device.newBufferWithLength_options(w * h * 4, MTLResourceOptions::StorageModeShared).context("read-back buffer")?;
    let cb = r.gfx.queue.commandBuffer().context("command buffer")?;
    let blit = cb.blitCommandEncoder().context("blit")?;
    // SAFETY: level 0 into a buffer of its size.
    unsafe {
        blit.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toBuffer_destinationOffset_destinationBytesPerRow_destinationBytesPerImage(
            t, 0, 0, MTLOrigin { x: 0, y: 0, z: 0 }, MTLSize { width: w, height: h, depth: 1 }, &buf, 0, w * 4, w * h * 4,
        );
    }
    blit.endEncoding();
    cb.commit();
    cb.waitUntilCompleted();
    // SAFETY: the copy has completed.
    let px = unsafe { std::slice::from_raw_parts(buf.contents().as_ptr() as *const u8, w * h * 4) };
    Ok((w as u32, h as u32, px.as_chunks::<4>().0.iter().flat_map(|p| [p[2], p[1], p[0], p[3]]).collect()))
}

impl ApplicationHandler for Handler {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.win.is_some() { return; }
        if let Err(e) = self.open(event_loop) {
            self.error = Some(e);
            event_loop.exit();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        match event {
            WindowEvent::CloseRequested => {
                self.app.ctx.res_mut::<crate::engine::app::AppExit>().0 = Some(0);
                event_loop.exit();
            }
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => self.sync_size(),
            WindowEvent::KeyboardInput { event, .. } => {
                let PhysicalKey::Code(code) = event.physical_key else { return };
                let mut input = self.app.ctx.res_mut::<InputState>();
                match event.state {
                    ElementState::Pressed => input.keys.press(code),
                    ElementState::Released => input.keys.release(code),
                }
            }
            WindowEvent::MouseInput { button, state, .. } => {
                let mut input = self.app.ctx.res_mut::<InputState>();
                match state {
                    ElementState::Pressed => input.mouse.press(button),
                    ElementState::Released => input.mouse.release(button),
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let d = match delta {
                    MouseScrollDelta::LineDelta(x, y) => glam::Vec2::new(x, y),
                    MouseScrollDelta::PixelDelta(p) => glam::Vec2::new(p.x as f32, p.y as f32) / 20.0,
                };
                self.app.ctx.res_mut::<InputState>().scroll.delta += d;
            }
            WindowEvent::Focused(f) => {
                let mut input = self.app.ctx.res_mut::<InputState>();
                if !f {
                    input.keys.release_all();
                    input.mouse.release_all();
                }
                drop(input);
                self.app.ctx.res_mut::<WindowState>().focused = f;
            }
            WindowEvent::RedrawRequested => {
                if let Err(e) = self.frame() {
                    self.error = Some(e);
                    event_loop.exit();
                    return;
                }
                if let Some(code) = self.app.exit_status() {
                    self.status = code;
                    event_loop.exit();
                }
            }
            _ => {}
        }
    }

    fn device_event(&mut self, _event_loop: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        if let DeviceEvent::MouseMotion { delta: (dx, dy) } = event {
            self.app.ctx.res_mut::<InputState>().motion.delta += glam::Vec2::new(dx as f32, dy as f32);
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(w) = self.win.as_ref() { w.window.request_redraw(); }
    }
}
