//! Bevy's `StandardMaterial`, reduced to the unlit case rc-engine uses (`shaders/shim/standard_unlit.wgsl`).

use super::material::{AlphaMode, BindGroup, Face, Material, PipelineDesc, Std140};
use crate::engine::assets::Handle;
use crate::engine::image::Image;
use crate::engine::mesh::MeshLayout;
use crate::engine::scene::Color;

#[derive(Clone, Debug)]
pub struct StandardMaterial {
    pub base_color: Color,
    pub base_color_texture: Option<Handle<Image>>,
    pub unlit: bool,
    pub alpha_mode: AlphaMode,
    pub cull_mode: Option<Face>,
    pub double_sided: bool,
    pub fog_enabled: bool,
}

impl Default for StandardMaterial {
    fn default() -> Self {
        StandardMaterial { base_color: Color::WHITE, base_color_texture: None, unlit: false, alpha_mode: AlphaMode::Opaque, cull_mode: Some(Face::Back), double_sided: false, fog_enabled: true }
    }
}

impl Material for StandardMaterial {
    fn shader() -> &'static str { "standard_unlit" }
    fn alpha_mode(&self) -> AlphaMode { self.alpha_mode }
    fn specialize(&self, desc: &mut PipelineDesc, _layout: &MeshLayout) -> Result<(), String> {
        desc.cull = self.cull_mode;
        Ok(())
    }
    fn bindings(&self, b: &mut BindGroup) {
        let mut s = Std140::default();
        s.vec4(glam::Vec4::from_array(self.base_color.to_linear())).vec4(glam::Vec4::new(self.base_color_texture.is_some() as u32 as f32, 0.0, 0.0, 0.0));
        b.uniform(0, s.finish());
        // Without a texture the material's own colour stands alone (flags.x = 0).
        if let Some(t) = &self.base_color_texture { b.texture_sampler(1, t); }
    }
    fn pipeline_key(&self) -> u64 { ((self.cull_mode.is_some() as u64) << 8) | matches!(self.alpha_mode, AlphaMode::Blend) as u64 }
}
