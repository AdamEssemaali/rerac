//! The on-screen pad (crate::engine::touch) drawn over the window: the renderer's `Hook::Window`, one instanced quad
//! per shape (shaders/shim/touch_controls.wgsl), premultiplied over the frame. Drawn while the pad is the device in
//! use (`InputState::touch_active`): from the start on iOS until a controller is used, and again after a touch.

use crate::engine::input::InputState;
use crate::engine::touch::Shape;
use crate::render::frame::HookCtx;
use crate::render::gfx::{self, Binding, Blend, PipelineKey, TopologyClass};
use objc2_metal::*;
use std::rc::Rc;

/// The shapes as the shader's `array<Shape>` (32 bytes each).
fn shape_bytes(shapes: &[Shape]) -> Vec<u8> {
    let mut out = Vec::with_capacity(shapes.len() * 32);
    for s in shapes {
        for f in [s.centre.x, s.centre.y, s.half.x, s.half.y] { out.extend_from_slice(&f.to_le_bytes()); }
        for u in [s.kind as u32, s.pressed as u32, 0, 0] { out.extend_from_slice(&u.to_le_bytes()); }
    }
    out
}

pub fn window_hook(hc: &mut HookCtx) -> anyhow::Result<()> {
    let input = hc.ctx.res::<InputState>();
    if !input.touch_active { return Ok(()); }
    let shapes = input.touch.shapes();
    drop(input);
    if shapes.is_empty() { return Ok(()); }
    let pipe = hc.gfx.pipeline(&PipelineKey {
        shader: "touch_controls",
        vertex_entry: "vertex",
        vertex_shader: None,
        fragment_entry: Some("fragment"),
        defs: Default::default(),
        vertex_attributes: vec![],
        vertex_stride: 0,
        color_formats: vec![hc.format],
        blend: Some(Blend::PREMULTIPLIED_ALPHA_BLENDING),
        write_mask: MTLColorWriteMask::All,
        depth: false,
        samples: 1,
        topology_class: TopologyClass::Triangle,
    })?;
    let (_, _, w, h) = hc.viewport;
    let screen = Binding::Bytes(Rc::new([w as f32, h as f32, 0.0, 0.0].iter().flat_map(|f| f.to_le_bytes()).collect()));
    let buf = Binding::Buffer(gfx::new_buffer(&hc.gfx.device, &shape_bytes(&shapes), "touch controls")?);
    let rpd = MTLRenderPassDescriptor::renderPassDescriptor();
    // SAFETY: attachment 0.
    let ca = unsafe { rpd.colorAttachments().objectAtIndexedSubscript(0) };
    ca.setTexture(Some(hc.color));
    ca.setLoadAction(MTLLoadAction::Load);
    ca.setStoreAction(MTLStoreAction::Store);
    let enc = hc.cb.renderCommandEncoderWithDescriptor(&rpd).ok_or_else(|| anyhow::anyhow!("touch controls: render encoder"))?;
    enc.setViewport(MTLViewport { originX: 0.0, originY: 0.0, width: w, height: h, znear: 0.0, zfar: 1.0 });
    enc.setRenderPipelineState(&pipe.state);
    gfx::bind(&enc, &pipe.vertex, true, &[(0, 0, &screen), (0, 1, &buf)]);
    if let Some(f) = &pipe.fragment { gfx::bind(&enc, f, false, &[(0, 1, &buf)]); }
    // SAFETY: four vertices (a strip) per shape, the shapes in the bound buffer.
    unsafe { enc.drawPrimitives_vertexStart_vertexCount_instanceCount(MTLPrimitiveType::TriangleStrip, 0, 4, shapes.len()) };
    enc.endEncoding();
    Ok(())
}
