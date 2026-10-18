//! Images (textures and render targets): the part of Bevy's `Image` API rc-engine uses.

use super::mesh::RenderAssetUsages;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum TextureFormat {
    Rgba8Unorm,
    Rgba8UnormSrgb,
    Bgra8Unorm,
    Bgra8UnormSrgb,
    Rgba16Float,
    R16Float,
    R8Unorm,
    Depth32Float,
}

impl TextureFormat {
    pub fn pixel_size(self) -> usize {
        match self {
            TextureFormat::R8Unorm => 1,
            TextureFormat::R16Float => 2,
            TextureFormat::Rgba16Float => 8,
            _ => 4,
        }
    }

    pub fn is_srgb(self) -> bool { matches!(self, TextureFormat::Rgba8UnormSrgb | TextureFormat::Bgra8UnormSrgb) }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextureDimension {
    D1,
    D2,
    D3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct Extent3d {
    pub width: u32,
    pub height: u32,
    pub depth_or_array_layers: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct TextureUsages(pub u32);

impl TextureUsages {
    pub const COPY_SRC: TextureUsages = TextureUsages(1);
    pub const COPY_DST: TextureUsages = TextureUsages(2);
    pub const TEXTURE_BINDING: TextureUsages = TextureUsages(4);
    pub const STORAGE_BINDING: TextureUsages = TextureUsages(8);
    pub const RENDER_ATTACHMENT: TextureUsages = TextureUsages(16);
    pub fn contains(self, o: TextureUsages) -> bool { self.0 & o.0 == o.0 }
}

impl std::ops::BitOr for TextureUsages {
    type Output = Self;
    fn bitor(self, o: Self) -> Self { TextureUsages(self.0 | o.0) }
}
impl std::ops::BitOrAssign for TextureUsages {
    fn bitor_assign(&mut self, o: Self) { self.0 |= o.0 }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TextureDescriptor {
    pub size: Extent3d,
    pub mip_level_count: u32,
    pub dimension: TextureDimension,
    pub format: TextureFormat,
    pub usage: TextureUsages,
    /// Other formats the texture may be viewed as (rc-engine's menu snapshot writes an sRGB image through a UNORM view).
    pub view_formats: &'static [TextureFormat],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum ImageAddressMode {
    #[default]
    ClampToEdge,
    Repeat,
    MirrorRepeat,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum ImageFilterMode {
    #[default]
    Nearest,
    Linear,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ImageSamplerDescriptor {
    pub address_mode_u: ImageAddressMode,
    pub address_mode_v: ImageAddressMode,
    pub address_mode_w: ImageAddressMode,
    pub mag_filter: ImageFilterMode,
    pub min_filter: ImageFilterMode,
    pub mipmap_filter: ImageFilterMode,
    pub anisotropy_clamp: u16,
}

impl Default for ImageSamplerDescriptor {
    fn default() -> Self {
        ImageSamplerDescriptor {
            address_mode_u: ImageAddressMode::ClampToEdge,
            address_mode_v: ImageAddressMode::ClampToEdge,
            address_mode_w: ImageAddressMode::ClampToEdge,
            mag_filter: ImageFilterMode::Nearest,
            min_filter: ImageFilterMode::Nearest,
            mipmap_filter: ImageFilterMode::Nearest,
            anisotropy_clamp: 1,
        }
    }
}

impl ImageSamplerDescriptor {
    pub fn linear() -> Self {
        ImageSamplerDescriptor { mag_filter: ImageFilterMode::Linear, min_filter: ImageFilterMode::Linear, mipmap_filter: ImageFilterMode::Linear, ..Default::default() }
    }

    pub fn nearest() -> Self { Self::default() }
}

/// Bevy's `ImageSampler`: the default sampler (Bevy's default is linear) or an explicit one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum ImageSampler {
    #[default]
    Default,
    Descriptor(ImageSamplerDescriptor),
}

impl ImageSampler {
    pub fn linear() -> Self { ImageSampler::Descriptor(ImageSamplerDescriptor::linear()) }
    pub fn nearest() -> Self { ImageSampler::Descriptor(ImageSamplerDescriptor::nearest()) }

    /// The descriptor this sampler stands for (Bevy's `ImagePlugin::default()` samples linearly).
    pub fn resolve(&self) -> ImageSamplerDescriptor {
        match self {
            ImageSampler::Default => ImageSamplerDescriptor::linear(),
            ImageSampler::Descriptor(d) => *d,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Image {
    /// Every mip level (and array layer) packed, level 0 first; `None` for a render target.
    pub data: Option<Vec<u8>>,
    pub texture_descriptor: TextureDescriptor,
    pub sampler: ImageSampler,
}

impl Default for Image {
    fn default() -> Self { Image::new_fill(Extent3d { width: 1, height: 1, depth_or_array_layers: 1 }, TextureDimension::D2, &[255; 4], TextureFormat::Rgba8UnormSrgb, RenderAssetUsages) }
}

impl Image {
    pub fn new(size: Extent3d, dimension: TextureDimension, data: Vec<u8>, format: TextureFormat, _usage: RenderAssetUsages) -> Self {
        let mut img = Image::new_uninit(size, dimension, format, RenderAssetUsages);
        img.data = Some(data);
        img
    }

    pub fn new_uninit(size: Extent3d, dimension: TextureDimension, format: TextureFormat, _usage: RenderAssetUsages) -> Self {
        Image {
            data: None,
            texture_descriptor: TextureDescriptor {
                size,
                mip_level_count: 1,
                dimension,
                format,
                usage: TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST,
                view_formats: &[],
            },
            sampler: ImageSampler::Default,
        }
    }

    /// Every texel set to `pixel` (one texel's bytes).
    pub fn new_fill(size: Extent3d, dimension: TextureDimension, pixel: &[u8], format: TextureFormat, _usage: RenderAssetUsages) -> Self {
        let n = (size.width * size.height * size.depth_or_array_layers.max(1)) as usize;
        let mut data = Vec::with_capacity(n * pixel.len());
        for _ in 0..n { data.extend_from_slice(pixel); }
        Image::new(size, dimension, data, format, RenderAssetUsages)
    }

    /// A render target (Bevy's `Image::new_target_texture`): no data, renderable and sampleable.
    pub fn new_target_texture(width: u32, height: u32, format: TextureFormat, _view_format: Option<TextureFormat>) -> Self {
        let mut img = Image::new_uninit(Extent3d { width, height, depth_or_array_layers: 1 }, TextureDimension::D2, format, RenderAssetUsages);
        img.texture_descriptor.usage = TextureUsages::TEXTURE_BINDING | TextureUsages::COPY_DST | TextureUsages::RENDER_ATTACHMENT;
        img
    }

    pub fn width(&self) -> u32 { self.texture_descriptor.size.width }
    pub fn height(&self) -> u32 { self.texture_descriptor.size.height }
    pub fn size(&self) -> glam::UVec2 { glam::UVec2::new(self.width(), self.height()) }

    pub fn is_render_target(&self) -> bool { self.texture_descriptor.usage.contains(TextureUsages::RENDER_ATTACHMENT) }

    /// Resizes a render target (Bevy's `Image::resize` for data-less images).
    pub fn resize(&mut self, size: Extent3d) {
        self.texture_descriptor.size = size;
        if let Some(d) = self.data.as_mut() {
            d.resize((size.width * size.height * size.depth_or_array_layers.max(1)) as usize * self.texture_descriptor.format.pixel_size(), 0);
        }
    }
}
