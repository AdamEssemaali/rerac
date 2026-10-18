//! The window (winit on a `CAMetalLayer`), input for the fly camera, and the headless screenshot path.

use crate::camera::FlyCam;
use crate::gpu::{self, Renderer};
use crate::level::Level;
use anyhow::{Context, Result};
use glam::Vec3;
use objc2::rc::{autoreleasepool, Retained};
use objc2::runtime::ProtocolObject;
use objc2_core_foundation::CGSize;
use objc2_metal::{MTLCommandBuffer, MTLCreateSystemDefaultDevice, MTLDrawable};
use objc2_quartz_core::{CAMetalDrawable, CAMetalLayer};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::collections::HashSet;
use std::path::Path;
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{Window, WindowId};

fn device() -> Result<Retained<gpu::Device>> { MTLCreateSystemDefaultDevice().context("no Metal device") }

/// `RC_SCREENSHOT`: one frame offscreen from the starting camera, as a PNG; no window.
pub fn screenshot(level: &Level, path: &Path) -> Result<()> {
    let (w, h) = std::env::var("RC_SCREENSHOT_SIZE")
        .ok()
        .and_then(|s| s.split_once('x').and_then(|(w, h)| Some((w.trim().parse().ok()?, h.trim().parse().ok()?))))
        .unwrap_or((1024usize, 768usize));
    let mut r = Renderer::new(device()?, level)?;
    let cam = FlyCam::new(level);
    let target = gpu::offscreen_target(&r.device, w, h)?;
    let cb = r.render(&cam, &target, None)?;
    cb.waitUntilCompleted();
    if let Some(e) = cb.error() { anyhow::bail!("Metal frame failed: {}", e.localizedDescription()); }
    let rgba = gpu::read_back(&r.device.clone(), &r, &target)?;
    let file = std::fs::File::create(path).with_context(|| format!("creating {}", path.display()))?;
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w as u32, h as u32);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()?.write_image_data(&rgba)?;
    println!("screenshot: {} ({w}x{h})", path.display());
    Ok(())
}

pub fn run(level: Level) -> Result<()> {
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App { level, state: None, error: None };
    event_loop.run_app(&mut app)?;
    app.error.map_or(Ok(()), Err)
}

struct App {
    level: Level,
    state: Option<State>,
    error: Option<anyhow::Error>,
}

struct State {
    window: Window,
    layer: Retained<CAMetalLayer>,
    renderer: Renderer,
    cam: FlyCam,
    keys: HashSet<KeyCode>,
    looking: bool,
    last: Instant,
    fps: (Instant, u32),
}

impl State {
    fn new(event_loop: &ActiveEventLoop, level: &Level) -> Result<Self> {
        let attrs = Window::default_attributes()
            .with_title(format!("ReRAC (Metal) — level {:02}", level.index))
            .with_inner_size(LogicalSize::new(1024.0, 768.0));
        let window = event_loop.create_window(attrs)?;
        let RawWindowHandle::AppKit(h) = window.window_handle()?.as_raw() else { anyhow::bail!("not an AppKit window") };
        // SAFETY: the NSView belongs to `window`, which outlives the layer's use (both live in State).
        let layer = unsafe { raw_window_metal::Layer::from_ns_view(h.ns_view) };
        // SAFETY: raw-window-metal hands out a CAMetalLayer, retained for us by `into_raw`.
        let layer: Retained<CAMetalLayer> = unsafe { Retained::from_raw(layer.into_raw().as_ptr().cast()) }.context("CAMetalLayer")?;
        let renderer = Renderer::new(device()?, level)?;
        layer.setDevice(Some(&renderer.device));
        layer.setPixelFormat(gpu::COLOR_FORMAT);
        layer.setFramebufferOnly(true);
        layer.setMaximumDrawableCount(3);
        if std::env::var("RC_NOVSYNC").is_ok_and(|v| v.trim() == "1") { layer.setDisplaySyncEnabled(false); }
        let mut s = State {
            cam: FlyCam::new(level),
            window,
            layer,
            renderer,
            keys: HashSet::new(),
            looking: false,
            last: Instant::now(),
            fps: (Instant::now(), 0),
        };
        s.resize();
        println!("controls: right mouse + move to look, WASD fly, Q/E down/up, Shift fast, P prints RC_CAM, Esc quits");
        Ok(s)
    }

    fn resize(&mut self) {
        let size = self.window.inner_size();
        self.layer.setContentsScale(self.window.scale_factor());
        self.layer.setDrawableSize(CGSize { width: size.width.max(1) as f64, height: size.height.max(1) as f64 });
    }

    fn frame(&mut self) -> Result<()> {
        let now = Instant::now();
        let dt = (now - self.last).as_secs_f32().min(0.1);
        self.last = now;
        let k = |c: KeyCode| self.keys.contains(&c) as i32 as f32;
        let dir = Vec3::new(k(KeyCode::KeyD) - k(KeyCode::KeyA), k(KeyCode::KeyE) - k(KeyCode::KeyQ), k(KeyCode::KeyW) - k(KeyCode::KeyS));
        let fast = self.keys.contains(&KeyCode::ShiftLeft) || self.keys.contains(&KeyCode::ShiftRight);
        self.cam.fly(dir, fast, dt);
        autoreleasepool(|_| -> Result<()> {
            let Some(drawable) = self.layer.nextDrawable() else { return Ok(()) };
            let target = drawable.texture();
            let present: &ProtocolObject<dyn MTLDrawable> = ProtocolObject::from_ref(&*drawable);
            self.renderer.render(&self.cam, &target, Some(present))?;
            Ok(())
        })?;
        self.fps.1 += 1;
        if self.fps.0.elapsed().as_secs_f32() >= 5.0 {
            println!("fps: {:.1}", self.fps.1 as f32 / self.fps.0.elapsed().as_secs_f32());
            self.fps = (Instant::now(), 0);
        }
        Ok(())
    }

    /// `RC_CAM` for the current view (game coordinates: eye and a point 10 units ahead).
    fn print_cam(&self) {
        let to_game = |v: Vec3| Vec3::new(v.x, -v.z, v.y);
        let (e, t) = (to_game(self.cam.eye), to_game(self.cam.eye + self.cam.forward() * 10.0));
        println!("RC_CAM={:.3},{:.3},{:.3},{:.3},{:.3},{:.3}", e.x, e.y, e.z, t.x, t.y, t.z);
    }
}

impl App {
    fn fail(&mut self, event_loop: &ActiveEventLoop, e: anyhow::Error) {
        self.error = Some(e);
        event_loop.exit();
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_some() { return; }
        match State::new(event_loop, &self.level) {
            Ok(s) => self.state = Some(s),
            Err(e) => self.fail(event_loop, e),
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(s) = self.state.as_mut() else { return };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(_) | WindowEvent::ScaleFactorChanged { .. } => s.resize(),
            WindowEvent::KeyboardInput { event, .. } => {
                let PhysicalKey::Code(code) = event.physical_key else { return };
                match event.state {
                    ElementState::Pressed => {
                        if code == KeyCode::Escape { event_loop.exit(); }
                        if code == KeyCode::KeyP && !event.repeat { s.print_cam(); }
                        s.keys.insert(code);
                    }
                    ElementState::Released => { s.keys.remove(&code); }
                }
            }
            WindowEvent::MouseInput { button: MouseButton::Right, state, .. } => s.looking = state == ElementState::Pressed,
            WindowEvent::MouseWheel { delta, .. } => {
                let y = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => y,
                    winit::event::MouseScrollDelta::PixelDelta(p) => p.y as f32 / 20.0,
                };
                s.cam.speed = (s.cam.speed * 1.1f32.powf(y)).clamp(0.5, 2000.0);
            }
            WindowEvent::Focused(false) => { s.keys.clear(); s.looking = false; }
            WindowEvent::RedrawRequested => {
                if let Err(e) = s.frame() { self.fail(event_loop, e); }
            }
            _ => {}
        }
    }

    fn device_event(&mut self, _event_loop: &ActiveEventLoop, _id: DeviceId, event: DeviceEvent) {
        let Some(s) = self.state.as_mut() else { return };
        if let DeviceEvent::MouseMotion { delta: (dx, dy) } = event {
            if s.looking { s.cam.look(dx as f32, dy as f32); }
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(s) = self.state.as_ref() { s.window.request_redraw(); }
    }
}
