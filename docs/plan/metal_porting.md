# Porting rc-engine modules to rc-metal (no Bevy)

Companion to `docs/plan/metal_renderer.md`. Every rc-engine module (`crates/rc-engine/src/<name>.rs`) becomes
`crates/rc-metal/src/<name>.rs` **with the same module name**, so `crate::<module>::<item>` paths keep working. The
logic, the doc comments, the constants, the tests and every game-facing behaviour stay **verbatim**; only Bevy goes.

## The runtime that replaces Bevy

| Bevy | rc-metal |
|---|---|
| `use bevy::prelude::*` and friends | `use crate::prelude::*;` (math is glam with the same names; `default()`, `FloatOrd`) |
| `#[derive(Resource / Component / Asset / TypePath / AsBindGroup / ShaderType)]` | delete the derive; any `'static` type is a resource or component |
| system `fn s(a: Res<A>, mut b: ResMut<B>, q: Query<..>, mut commands: Commands)` | `pub fn s(ctx: &mut Ctx)` (same name, `pub`) |
| `Res<T>` / `ResMut<T>` / `Option<Res<T>>` | `ctx.res::<T>()` / `ctx.res_mut::<T>()` / `ctx.try_res::<T>()` / `ctx.try_res_mut::<T>()` (run-time borrows: never hold a `res_mut` of T while something else borrows T) |
| `Query<(Entity, &A, &mut B), (With<C>, Without<D>)>` | `for (e, a, mut b) in ctx.world.query::<(Entity, &A, &mut B)>().with::<&C>().without::<&D>().iter()` (hecs 0.11: put `Entity` in the tuple to get it) |
| `q.get(e)` / `q.get_mut(e)` | `ctx.world.get::<&A>(e)` / `ctx.world.get::<&mut A>(e)` (→ `Result<Ref/RefMut>`); several components: `ctx.world.query_one::<(&A, &B)>(e)` |
| `Single<..>` / `q.iter().next()` | `ctx.world.query::<..>().iter().next()` |
| `commands.spawn(bundle)` (`.id()`) | `ctx.world.spawn(bundle)` returns the `Entity` (immediate; do it when no query of the world is alive: collect first, spawn after) |
| `commands.entity(e).insert(c)` / `.remove::<C>()` / `.despawn()` | `ctx.world.insert_one(e, c)` (or `insert(e, (a, b))`) / `ctx.world.remove_one::<C>(e)` / `despawn_recursive(&mut ctx.world, e)` (all `Result`: `let _ =`) |
| deferred `commands.queue(..)` | `ctx.commands().add(move \|world, res\| ..)` (applied right after the system) |
| `commands.insert_resource(x)` / `remove_resource::<T>()` | `ctx.res.insert(x)` / `ctx.res.remove::<T>()` |
| spawning while holding a resource | borrow resources through the field, not the method: `let mut images = ctx.res.get_mut::<Assets<Image>>(); let e = ctx.world.spawn(..);` (`ctx.res_mut()` borrows all of `ctx`) |
| `Local<T>` | a private resource type (`struct SysLocal(T)`, inserted in the module's `plugin`), or a `static` (`Mutex`, atomics) |
| `x.is_changed()` on a resource | keep the last value seen (in a local) and compare; `LevelGeneration`: remember the last generation number |
| `Time<Real>` / `Time<Virtual>` / `Time<Fixed>` / `Time` | `ctx.res::<Time>().real` / `.virt` / `.fixed` / `.default` (each a `Clock`: `delta_secs()`, `elapsed_secs()`, …); `timestep()` |
| `ButtonInput<KeyCode>` / `ButtonInput<MouseButton>` / `AccumulatedMouseMotion` / `AccumulatedMouseScroll` / `Query<&Gamepad>` | `ctx.res::<InputState>()` → `.keys` / `.mouse` / `.motion` / `.scroll` / `.first_gamepad()` (same method names; `KeyCode`, `MouseButton`, `GamepadButton` have Bevy's variant names) |
| `Query<&mut Window, With<PrimaryWindow>>` | `ctx.res_mut::<Window>()` (fields: `physical_size`, `scale_factor`, `fullscreen`, `vsync`, `cursor_grabbed`, `cursor_visible`, `refresh_millihertz`) |
| `info!` / `warn!` / `error!` | same macros (crate root) |
| `AppExit` messages | `ctx.res_mut::<AppExit>().0 = Some(code)` |
| `Assets<T>`, `Handle<T>`, `Mesh`, `Image`, `ShaderBuffer`, `Mesh3d`, `MeshMaterial3d<M>`, `MeshTag`, `Transform`, `GlobalTransform`, `Visibility`, `InheritedVisibility`, `ViewVisibility`, `RenderLayers`, `Name`, `ChildOf`, `NoFrustumCulling`, `Color`, `Camera`, `Camera3d`, `Camera2d`, `Projection`, `Msaa`, `ClearColor(Config)`, `RenderTarget` | same names and the same methods rc-engine calls (crate::engine, crate::render). `Image::data`, `.texture_descriptor`, `.sampler` as in Bevy |
| `with_children` / `add_children(&[..])` / `add_child` | spawn the children with `ChildOf(parent)` |
| `bevy::asset::RenderAssetUsages`, `bevy::mesh::*`, `bevy::image::*`, `bevy::render::render_resource::{TextureFormat, Extent3d, …}` | crate::prelude (same names) |
| `Camera3d { depth_load_op: Camera3dDepthLoadOp::Clear(v) / Load }` | component `Camera3dDepth::Clear(v)` / `Camera3dDepth::Load` next to `Camera3d` |
| `CameraOutputMode::Skip` | component `OutputSkip` (informational) |
| `IsDefaultUiCamera`, `UiTargetCamera`, `Node`, `Val`, `PositionType`, `ImageNode`, `MaterialNode<M>`, `BackgroundColor`, `GlobalZIndex`, `ZIndex` | crate::render::ui (absolute layout only: left / top / width / height) |
| `Tonemapping`, `DebandDither`, `Hdr`, `NoIndirectDrawing`, `ClusterConfig`, observers on `Camera3d` | drop (the renderer does none of it) |
| `Sprite` | crate::render::frame::Sprite (`image`, `custom_size`, `color`) |

`Ctx` fields: `world` (hecs `World`), `res` (`Resources`). `ctx.world.query` borrows the world immutably (components
are borrow-checked per type at run time, like Bevy queries); `ctx.world.spawn / insert / despawn` need `&mut`.

## Plugins and the schedule

A module's `Plugin` becomes `pub fn plugin(app: &mut crate::engine::app::App, r: &mut crate::render::frame::Renderer)`:
it inserts the module's resources (`app.ctx.res.insert(..)` / `.init::<T>()`) and registers its render types:
* `r.add_material::<M>(&mut app.ctx)` (Bevy's `MaterialPlugin::<M>`), `r.add_material_2d::<M>(&mut app.ctx)`;
* `crate::render::ui::add_ui_material::<M>(&mut app.ctx)` (Bevy's `UiMaterialPlugin::<M>`).

**Systems are not registered by the module.** Keep every system function with its rc-engine name, `pub`, and the
central schedule in `main.rs` calls them in the order of the schedule map (rc-engine's `.after` / `.before` / sets /
`run_if`). A `run_if` that is a plugin-level condition (e.g. `RC_HUD=0` skips the HUD plugin) stays a `pub fn` the
schedule can call. In your report, list every system with its rc-engine schedule and constraints.

## Materials

Implement `crate::render::material::Material` (3D), `crate::render::frame::Material2d` (2D) or
`crate::render::ui::UiMaterial` (UI):
* `fn shader() -> &'static str`: the WGSL file stem (`"tfrag"` for `shaders/tfrag.wgsl`); the WGSL files are shared
  with rc-engine and translated by naga (crate::shader); the material group is `@group(2)` (Bevy's
  `#{MATERIAL_BIND_GROUP}`), the view `@group(0)`, the mesh instances `@group(1)`; UI materials use group 1 as Bevy's.
* `alpha_mode`, `depth_bias` as in rc-engine.
* `specialize(&self, desc: &mut PipelineDesc, layout: &MeshLayout) -> Result<(), String>`:
  `descriptor.primitive.cull_mode = None` → `desc.cull = None`; `key.bind_group_data.specialize(descriptor)` →
  `self.pass.specialize(desc)` (crate::gs_state); `descriptor.vertex.buffers = vec![layout.0.get_layout(&[A.at_shader_location(n), ..])?]`
  → `desc.vertex_attributes = vec![A.at_shader_location(n), ..]`; shader defs → `desc.def("X")` / `desc.def_u("X", v)`
  / `desc.def_i(..)`; entry points → `desc.vertex_entry` / `desc.fragment_entry`; target blend → `desc.blend`;
  `crate::display_blend::specialize(desc)`.
* `pipeline_key(&self) -> u64`: everything `specialize` reads from `self` (Bevy's `bind_group_data`, e.g.
  `self.pass.key()`), so pipelines are cached per key.
* `bindings(&self, b: &mut BindGroup)`: one call per `AsBindGroup` attribute, same numbers: `#[texture(n)]` →
  `b.texture(n, &self.tex)`, `#[sampler(n)]` → `b.sampler(n, &self.tex)` (the image's own sampler), `#[storage(n, ..)]` →
  `b.storage(n, &self.buf)`, `#[uniform(n)] field: T` → `b.uniform(n, bytes)` with the bytes in WGSL uniform layout
  (`Std140`: `.vec4(..)`, `.f32(..)`, `.u32(..)`, `.mat4(..)`, then `.finish()`), fields in declaration order.

`ShaderType` uniform structs keep their fields; add a `fn bytes(&self) -> Vec<u8>` (Std140) next to them.

## Render-world code

The render world is gone; its jobs map to renderer extension points (crate::render::frame):
* **Full-screen passes** (rc-engine `gs_post`, Bevy's `FullscreenMaterial`): give the pass as
  `pub fn fullscreen_pass() -> FullscreenPass { shader, entry: "fragment", defs, uniform: fn(&World, Entity) -> Option<Vec<u8>>, blend, after_ui }`;
  `uniform` returns the component's bytes when the camera carries it (bindings: 0 = the frame, 1 = sampler (nearest), 2 = the uniform).
  The central code registers them in rc-engine's order.
* **Passes inside a view** (rc-engine's shadow volumes): `pub fn ..._hook(hc: &mut HookCtx) -> anyhow::Result<()>`, called
  for every 3D camera after the opaque / alpha-mask / pre-shadow draws (`Hook::AfterOpaque`). `HookCtx` has `gfx`
  (`crate::render::gfx::Gfx`: pipelines from WGSL, buffers, textures, samplers), `ctx` (the world, read-only), `cb` (the
  frame's command buffer: begin your own render passes on it), `color` / `depth` textures (load them), the view
  uniform bytes, the viewport, the camera entity, MSAA samples and the target format. Bind with `crate::render::gfx::bind`.
* **After all cameras** (rc-engine's menu snapshot copy): `Hook::Finish`.
* Effects (`DisplayEffect`), pre-shadow ordering (`BeforeShadows`, `WorldDrawOrder`) and the stable transparent sort are
  done by the renderer: keep inserting those components; their render-world systems are gone.

## Don'ts

* Do not edit `crates/rc-metal/src/{engine,render}/`, `shader.rs`, `prelude.rs`, `main.rs`, `Cargo.toml` or another
  group's modules. If the runtime lacks something, add a private helper in your module, or write it down in your
  report (what, why) and code against the API you propose.
* Do not change behaviour, constants or comments that describe the game. Where a comment describes a Bevy mechanism
  that no longer exists, adjust it to the rc-metal equivalent in one sentence.
* Do not touch `crates/rc-engine`.
