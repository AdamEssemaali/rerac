// rc-metal's own: the on-screen pad of crate::engine::touch, drawn by crate::touch_controls over the window. One
// instance per shape (a quad around it); the fragment draws the shape as a signed distance, anti-aliased over a pixel.

struct Screen {
    size: vec2<f32>,
    _pad: vec2<f32>,
}

struct Shape {
    centre: vec2<f32>,
    half: vec2<f32>,
    // crate::engine::touch::ShapeKind.
    kind: u32,
    pressed: u32,
    _pad: vec2<u32>,
}

@group(0) @binding(0) var<uniform> screen: Screen;
@group(0) @binding(1) var<storage, read> shapes: array<Shape>;

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    // Pixels from the shape's centre.
    @location(0) local: vec2<f32>,
    @location(1) @interpolate(flat) index: u32,
}

@vertex
fn vertex(@builtin(vertex_index) v: u32, @builtin(instance_index) i: u32) -> VertexOutput {
    let s = shapes[i];
    let corner = vec2<f32>(f32(v & 1u), f32(v >> 1u)) * 2.0 - 1.0;
    let extent = s.half + vec2<f32>(2.0);
    let p = s.centre + corner * extent;
    var out: VertexOutput;
    out.position = vec4<f32>(p.x / screen.size.x * 2.0 - 1.0, 1.0 - p.y / screen.size.y * 2.0, 0.0, 1.0);
    out.local = corner * extent;
    out.index = i;
    return out;
}

fn box(p: vec2<f32>, b: vec2<f32>, r: f32) -> f32 {
    let q = abs(p) - b + vec2<f32>(r);
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - r;
}

fn segment(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>) -> f32 {
    let pa = p - a;
    let ba = b - a;
    let h = clamp(dot(pa, ba) / dot(ba, ba), 0.0, 1.0);
    return length(pa - ba * h);
}

// Coverage of the inside of distance `d` (pixels).
fn cover(d: f32) -> f32 { return clamp(0.5 - d, 0.0, 1.0); }

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let s = shapes[in.index];
    let p = in.local;
    let r = s.half.x;
    let on = s.pressed != 0u;
    var fill = select(0.18, 0.38, on);
    var edge = select(0.45, 0.8, on);
    var glyph_colour = vec3<f32>(1.0);
    var glyph = 0.0;
    var d = length(p) - r;
    let stroke = max(r * 0.09, 1.5);
    switch s.kind {
        // Stick base: a faint disc and its rim.
        case 0u: { fill = select(0.08, 0.14, on); }
        // Stick knob.
        case 1u: { fill = select(0.3, 0.5, on); }
        case 2u: {
            let k = r * 0.38;
            glyph = cover(min(segment(p, vec2<f32>(-k, -k), vec2<f32>(k, k)), segment(p, vec2<f32>(-k, k), vec2<f32>(k, -k))) - stroke * 0.5);
            glyph_colour = vec3<f32>(0.55, 0.7, 1.0);
        }
        case 3u: {
            glyph = cover(abs(length(p) - r * 0.4) - stroke * 0.5);
            glyph_colour = vec3<f32>(1.0, 0.45, 0.45);
        }
        case 4u: {
            glyph = cover(abs(box(p, vec2<f32>(r * 0.36), 0.0)) - stroke * 0.5);
            glyph_colour = vec3<f32>(1.0, 0.6, 0.9);
        }
        case 5u: {
            let a = vec2<f32>(0.0, -r * 0.45);
            let b = vec2<f32>(r * 0.42, r * 0.3);
            let c = vec2<f32>(-r * 0.42, r * 0.3);
            glyph = cover(min(min(segment(p, a, b), segment(p, b, c)), segment(p, c, a)) - stroke * 0.5);
            glyph_colour = vec3<f32>(0.45, 1.0, 0.7);
        }
        // Shoulder / START / SELECT: a pill.
        default: { d = box(p, s.half, s.half.y); }
    }
    let inside = cover(d);
    let rim = cover(abs(d + stroke * 0.5) - stroke * 0.5);
    let a = max(max(inside * fill, rim * edge), glyph * 0.9 * inside);
    let c = mix(vec3<f32>(1.0), glyph_colour, glyph);
    // Premultiplied.
    return vec4<f32>(c * a, a);
}
