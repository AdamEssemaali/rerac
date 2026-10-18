//! `rerac-metal`: ReRAC's runtime on its own native Metal renderer, without Bevy (docs/plan/metal_renderer.md). The game
//! modules are rc-engine's, ported to rc-metal's runtime (docs/plan/metal_porting.md) under the same names; this file is
//! rc-engine's `main.rs`: the arguments, the boot level load, the plugins, and the schedule that rc-engine expressed
//! with Bevy's system ordering, here written out in order (docs/plan/metal_renderer.md, "Schedule").
//!
//! Arguments and environment: as rc-engine's (`--data-dir`, `--version-json`, `RC_*`; crate::disc_source and the
//! module docs). `RC_SCREENSHOT` saves the game frame after `RC_SCREENSHOT_DELAY` seconds (default 3) and 30 frames, then
//! exits; with `RC_SCREENSHOT_FRAME` the capture is frame-exact instead (crate::determinism).

#[macro_use]
mod log;

mod aa_blit;
mod afterimage_render;
#[cfg(target_os = "macos")]
mod app;
mod asset_write;
mod audio_out;
mod crash_log;
mod determinism;
mod disc_source;
mod display;
mod display_blend;
mod engine;
mod flight_render;
mod fly_cam;
mod fog_state;
mod frame_log;
mod frame_pace;
mod fx_draw;
mod game_camera;
mod gameplay;
mod graphics;
mod gs_post;
mod gs_state;
mod hud_images;
mod hud_render;
mod input_map;
mod interact_render;
mod level_load;
mod level_switch;
mod marker_render;
mod media_render;
mod menu_models;
mod menu_render;
mod mirror_render;
mod moby_anim;
mod moby_attach;
mod moby_light;
mod moby_lod;
mod moby_render;
mod moby_spawn;
mod movie_render;
mod occlusion;
mod particle_render;
mod play_camera;
mod pre_shadow;
mod prelude;
mod reactive_render;
#[cfg(target_os = "macos")]
mod render;
mod render_settings;
mod reticle_render;
mod saves;
mod scene_render;
mod screen_canvas;
mod screen_tint;
mod sea_render;
mod shader;
mod shadow_render;
mod shrub_billboard;
mod shrub_light;
mod shrub_render;
mod sky_render;
mod sky_stars;
mod tesla_render;
mod text_render;
mod tfrag_light;
mod tfrag_lod;
mod tfrag_render;
mod thruster_render;
mod tie_light;
mod tie_lod;
mod tie_render;
mod title_world;
mod travel_render;
mod vendor_render;
mod visibomb_view;
mod walloper_render;
mod water_render;
mod world_lights;

use crate::engine::app::{App, Stage, SystemFn, PROPAGATE};
use crate::level_switch::{remove, reset};
use crate::prelude::*;
use fly_cam::FlyCam;
use level_load::LoadedLevel;
use std::path::PathBuf;
use std::time::Duration;
use tfrag_render::game_to_bevy;

/// The loaded level (rc-engine's `Level` resource).
pub struct Level(pub LoadedLevel);

struct ScreenshotRequest {
    path: PathBuf,
    /// Minimum wall time and frame count before capturing, so everything has been uploaded.
    delay: Duration,
    min_frames: u32,
    frames: u32,
    requested: bool,
    start: std::time::Instant,
}

#[cfg(not(target_os = "macos"))]
fn main() {
    eprintln!("rerac-metal runs on macOS only (Metal); use rerac (rc-engine) elsewhere");
    std::process::exit(1);
}

#[cfg(target_os = "macos")]
fn main() -> anyhow::Result<()> {
    crash_log::install();
    // `--version-json` exits here; a missing or wrong data folder exits with a clear error (crate::disc_source).
    let root = disc_source::startup();
    let index = level_load::level_index();
    // The one level load (the boot's and every runtime level change's, crate::level_switch): the level data, the loader's
    // ship and the load-time update pass (crate::moby_spawn; RC_SPAWN_RULES=0 skips it), the fog zones and the
    // underwater test (crate::fog_state). The boot's game state first: the level's spawn test reads its save bits
    // (crate::gameplay::level_spawn_save).
    let boot = gameplay::boot_state(&root, index);
    let save = gameplay::level_spawn_save(boot.as_ref().map(|(gs, _)| gs), index);
    let level_switch::LevelBundle { mut level, spawn, fog: fog_state, .. } = level_switch::load_bundle(&root, index, &save)?;
    let t = &level.timings;
    let bg = level.background;
    println!(
        "level {index:02} from {}: {} tfrags, {} textures; load {:.1} ms (read {:.1}, WAD {:.1}, core {:.1}, tfrags {:.1}, textures {:.1})",
        root.display(), level.tfrags.len(), level.textures.len(), ms(t.total()),
        ms(t.read), ms(t.decompress), ms(t.parse_core), ms(t.parse_tfrags), ms(t.parse_textures)
    );
    println!("background (GS clear colour): {bg:?}");
    println!("{}", fly_cam::CONTROLS);

    let mut renderer = render::frame::Renderer::new()?;
    renderer.ui = Some(render::ui::draw);
    let mut app = App::new();
    let r = &mut renderer;

    // Plugins, in rc-engine's order.
    display::plugin(&mut app);
    determinism::plugin(&mut app);
    frame_pace::plugin(&mut app);
    graphics::plugin(&mut app);
    render_settings::plugin(&mut app, &r.gfx.device);
    r.add_material::<tfrag_render::TfragMaterial>(&mut app.ctx);
    // Bevy's DefaultPlugins register StandardMaterial (crate::moby_attach's rope uses it).
    r.add_material::<StandardMaterial>(&mut app.ctx);
    fly_cam::plugin(&mut app);
    game_camera::plugin(&mut app, level.fog);
    occlusion::plugin(&mut app, r);
    moby_render::plugin(&mut app, r);
    moby_anim::plugin(&mut app, r);
    moby_spawn::plugin(&mut app, r);
    moby_attach::plugin(&mut app, r);
    shadow_render::plugin(&mut app, r);
    frame_log::plugin(&mut app, r);
    app.ctx.res.insert(spawn);
    tie_render::plugin(&mut app, r);
    shrub_render::plugin(&mut app, r);
    world_lights::plugin(&mut app, r);
    sky_render::plugin(&mut app, r);
    particle_render::plugin(&mut app, r);
    if water_render::enabled() { water_render::plugin(&mut app, r); }
    if sea_render::enabled() { sea_render::plugin(&mut app, r); }
    fx_draw::plugin(&mut app, r);
    afterimage_render::plugin(&mut app, r);
    reticle_render::plugin(&mut app, r);
    tesla_render::plugin(&mut app, r);
    walloper_render::plugin(&mut app, r);
    reactive_render::plugin(&mut app, r);
    sky_stars::plugin(&mut app, r);
    fog_state::plugin(&mut app, r);
    screen_tint::plugin(&mut app, r);
    visibomb_view::plugin(&mut app, r);
    hud_render::plugin(&mut app, r);
    gameplay::plugin(&mut app, r, boot);
    menu_render::plugin(&mut app, r);
    screen_canvas::plugin(&mut app, r);
    vendor_render::plugin(&mut app, r);
    menu_models::plugin(&mut app, r);
    scene_render::plugin(&mut app);
    movie_render::plugin(&mut app);
    mirror_render::plugin(&mut app, r);
    aa_blit::plugin(&mut app, r);
    audio_out::plugin(&mut app, level.audio.take());
    app.ctx.res.insert(fog_state);
    // The GS frame clear: level settings +0x00..+0x08 via `set_background_color` (game_camera::level_background).
    // srgb_u8: the bytes are display-encoded like every GS colour; the sRGB target stores them back exactly.
    app.ctx.res.insert(ClearColor(Color::srgb_u8(bg[0], bg[1], bg[2])));
    app.ctx.res.insert(Level(level));
    level_switch::plugin(&mut app);
    travel_render::plugin(&mut app);
    flight_render::plugin(&mut app);
    title_world::plugin(&mut app);
    app.ctx.res.insert(FpsLocal::default());
    if let Some(path) = std::env::var_os("RC_SCREENSHOT").filter(|_| determinism::screenshot_frame().is_none()) {
        let secs = std::env::var("RC_SCREENSHOT_DELAY").ok().and_then(|s| s.parse().ok()).unwrap_or(3.0);
        app.ctx.res.insert(ScreenshotRequest {
            path: path.into(),
            delay: Duration::from_secs_f32(secs),
            min_frames: 30,
            frames: 0,
            requested: false,
            start: std::time::Instant::now(),
        });
    }

    // Renderer extension points: the full-screen passes in rc-engine's Core3d order (mirror ‖ screen tint, the AA blit
    // and display copy, the UI pass, the missile view, the underwater tint, the scene fade), the in-view and final hooks.
    r.fullscreen.0.push(mirror_render::mirror_fullscreen_pass());
    r.fullscreen.0.push(screen_tint::screen_tint_fullscreen_pass());
    r.fullscreen.0.push(aa_blit::blit_across_fullscreen_pass());
    r.fullscreen.0.push(aa_blit::blit_down_fullscreen_pass());
    r.fullscreen.0.push(aa_blit::copy_across_fullscreen_pass());
    r.fullscreen.0.push(aa_blit::copy_down_fullscreen_pass());
    r.fullscreen.0.push(visibomb_view::missile_view_fullscreen_pass());
    r.fullscreen.0.push(fog_state::underwater_fullscreen_pass());
    r.fullscreen.0.push(scene_render::scene_fade_fullscreen_pass());
    r.add_hook(Hook::AfterOpaque, shadow_render::shadow_hook);
    r.add_hook(Hook::Finish, menu_render::snapshot_hook);

    schedule(&mut app);
    let status = app::run(app, renderer)?;
    std::process::exit(status);
}

fn ms(d: Duration) -> f64 { d.as_secs_f64() * 1e3 }

/// rc-engine's system order (docs/plan/metal_renderer.md, "Schedule"): each stage lists its systems in an order that
/// satisfies every `.after` / `.before` / set constraint of rc-engine; where Bevy left two systems unordered the choice
/// is noted.
fn schedule(app: &mut App) {
    let play = gameplay::enabled();
    let hud = hud_render::enabled();
    let water = water_render::enabled();
    let sea = sea_render::enabled();
    let mut add = |stage: Stage, list: &[(&'static str, SystemFn)]| {
        for &(n, f) in list { app.add(stage, n, f); }
    };

    add(Stage::Startup, &[
        ("display::spawn_present", display::spawn_present),
        ("tfrag_lod::no_vsync", tfrag_lod::no_vsync),
        ("movie_render::setup", movie_render::setup),
        ("audio_out::start_output", audio_out::start_output),
        ("main::setup_camera", setup_camera),
        ("display::redirect", display::redirect),
    ]);
    add(Stage::PostStartup, &[("level_switch::run_level_startup", level_switch::run_level_startup)]);

    // Level start (rc-engine: all unordered, commands applied at the end of the schedule; here each system's at once).
    add(Stage::LevelStartup, &[
        ("main::setup", setup),
        ("occlusion::init", occlusion::init),
        ("moby_render::spawn_system", moby_render::spawn_system),
        ("shadow_render::setup", shadow_render::setup),
        ("tie_render::spawn_system", tie_render::spawn_system),
        ("shrub_render::spawn_system", shrub_render::spawn_system),
        ("shrub_billboard::spawn_system", shrub_billboard::spawn_system),
        ("sky_render::spawn_sky", sky_render::spawn_sky),
        ("particle_render::setup", particle_render::setup),
    ]);
    if water {
        add(Stage::LevelStartup, &[("water_render::setup", water_render::setup), ("water_render::fire_field_setup", water_render::fire_field_setup)]);
    }
    if hud { add(Stage::LevelStartup, &[("hud_render::setup", hud_render::setup)]); }
    add(Stage::LevelStartup, &[("display::redirect", display::redirect)]);
    add(Stage::LevelPostStartup, &[
        ("moby_spawn::init", moby_spawn::init),
        ("sky_render::main_camera_loads", sky_render::main_camera_loads),
        ("sky_stars::setup", sky_stars::setup),
    ]);

    add(Stage::LevelUnload, &[
        ("occlusion::unload", occlusion::unload),
        ("tfrag_lod: remove TfragLodState", remove::<tfrag_lod::TfragLodState>),
        ("moby_render: reset PointLightFrame", reset::<moby_render::PointLightFrame>),
        ("moby_render: remove MobyOcclusion", remove::<moby_render::MobyOcclusion>),
        ("moby_render: remove MobyAnim", remove::<moby_anim::MobyAnim>),
        ("moby_attach: remove MobyAttach", remove::<moby_attach::MobyAttach>),
        ("shadow_render: reset ShadowVolumes", reset::<shadow_render::ShadowVolumes>),
        ("shadow_render: remove ShadowGame", remove::<shadow_render::ShadowGame>),
        ("tie_lod::unload", tie_lod::unload),
        ("shrub_render::unload", shrub_render::unload),
        ("sky_render::unload", sky_render::unload),
        ("particle_render: remove ParticleSim", remove::<particle_render::ParticleSim>),
        ("particle_render: remove ParticleMesh", remove::<particle_render::ParticleMesh>),
        ("fx_draw: reset FxDraw", reset::<fx_draw::FxDraw>),
        ("reticle_render: reset", reset::<reticle_render::ReticleDraw>),
        ("tesla_render: reset", reset::<tesla_render::TeslaDraw>),
        ("walloper_render: reset", reset::<walloper_render::WalloperDraw>),
        ("reactive_render: reset", reset::<reactive_render::ReactiveDraw>),
        ("sky_stars::unload", sky_stars::unload),
        ("hud_render::level_unload", hud_render::level_unload),
        ("menu_render::level_unload", menu_render::level_unload),
        ("screen_canvas::reset", screen_canvas::reset),
        ("menu_models::level_unload", menu_models::level_unload),
        ("scene_render: reset ActiveScene", reset::<scene_render::ActiveScene>),
        ("scene_render: reset FadeHold", reset::<scene_render::FadeHold>),
        ("scene_render::level_reset", scene_render::level_reset),
        ("flight_render::unload", flight_render::unload),
        ("title_world::unload", title_world::unload),
    ]);
    if water { add(Stage::LevelUnload, &[("water_render::unload", water_render::unload)]); }
    if sea { add(Stage::LevelUnload, &[("sea_render::unload", sea_render::unload)]); }
    if play {
        add(Stage::LevelUnload, &[
            ("gameplay: remove Play", remove::<gameplay::Play>),
            ("gameplay::despawn_lod_parents", gameplay::despawn_lod_parents),
            ("gameplay: remove AmmoTable", remove::<gameplay::AmmoTable>),
            ("gameplay: remove HeldWeapon", remove::<gameplay::HeldWeapon>),
            ("gameplay: remove PlayView", remove::<play_camera::PlayView>),
            ("gameplay: reset TickBudget", reset::<gameplay::TickBudget>),
            ("vendor_render: remove VendorGfx", remove::<vendor_render::VendorGfx>),
            ("travel_render::level_unload", travel_render::level_unload),
        ]);
    }

    // First, PreUpdate, FixedUpdate, AfterFixedMainLoop, Update, PostUpdate: rc-engine's executable order (Bevy's
    // topological sort of its schedules, dumped from a frame-exact Novalis run; systems absent there placed by their
    // constraints). Where Bevy's order satisfies no constraint between two systems it is still the order the conflicting
    // ones ran in.
    add(Stage::First, &[
        ("frame_pace::steps", frame_pace::steps),
        ("determinism::pattern_steps", determinism::pattern_steps),
        ("display::sync_frame", display::sync_frame),
        ("time", engine::app::time_system),
        ("determinism::advance_ticks", determinism::advance_ticks),
        ("visibomb_view::short_far", visibomb_view::short_far),
    ]);
    if play { add(Stage::First, &[("menu_render::clear_snapshot_request", menu_render::clear_snapshot_request)]); }

    add(Stage::PreUpdate, &[("graphics::apply_detail", graphics::apply_detail)]);
    if play {
        add(Stage::PreUpdate, &[
            ("menu_render::setup", menu_render::setup),
            ("vendor_render::setup", vendor_render::setup),
            ("menu_models::setup", menu_models::setup),
        ]);
    }
    add(Stage::PreUpdate, &[
        ("graphics::apply_samples", graphics::apply_samples),
        ("moby_attach::setup", moby_attach::setup),
    ]);
    if play { add(Stage::PreUpdate, &[("gameplay::setup", gameplay::setup)]); }
    add(Stage::PreUpdate, &[("determinism::no_live_input", determinism::no_live_input)]);
    if play {
        add(Stage::PreUpdate, &[
            ("input_map::sample_system", input_map::sample_system),
            ("gameplay::set_budget", gameplay::set_budget),
            ("gameplay::keys", gameplay::keys),
        ]);
    }

    let mut fixed: Vec<(&'static str, SystemFn)> = vec![
        ("audio_out::sync_underwater", audio_out::sync_underwater),
        ("audio_out::sync_options", audio_out::sync_options),
        ("movie_render::movie_exit", movie_render::movie_exit),
    ];
    if water { fixed.push(("water_render::tick", water_render::tick)); }
    fixed.extend_from_slice(&[
        ("scene_render::fade_step", scene_render::fade_step),
        ("scene_render::scene_frame", scene_render::scene_frame),
    ]);
    if play {
        fixed.extend_from_slice(&[
            ("menu_render::return_page_reopen", menu_render::return_page_reopen),
            ("travel_render::pre_tick", travel_render::pre_tick),
            ("gameplay::tick (GameTick)", game_tick),
        ]);
    }
    fixed.extend_from_slice(&[
        ("scene_render::fade_take", scene_render::fade_take),
        ("fog_state::hero_env_lighting", fog_state::hero_env_lighting),
        ("movie_render::movie_frame", movie_render::movie_frame),
        ("audio_out::scene_sound", audio_out::scene_sound),
        ("scene_render::actor_mobys", scene_render::actor_mobys),
        ("particle_render::tick", particle_render::tick),
        ("sky_stars::star_frame", sky_stars::star_frame),
    ]);
    if play {
        fixed.extend_from_slice(&[
            ("travel_render::post_tick", travel_render::post_tick),
            ("menu_render::menu_frame", menu_render::menu_frame),
            ("title_world::tick", title_world::tick),
            ("gameplay::elapsed_frame", gameplay::elapsed_frame),
            ("menu_render::map_trace", menu_render::map_trace),
            ("shadow_render::shadow_tick", shadow_render::shadow_tick),
            ("saves::card_frame", saves::card_frame),
        ]);
    }
    // `moby_anim::tick`: last (rc-engine: unordered against the game tick; both write MobyAnim).
    fixed.push(("moby_anim::tick", moby_anim::tick));
    add(Stage::FixedUpdate, &fixed);
    add(Stage::FixedPostUpdate, &[("moby_attach::update", moby_attach::update)]);
    if play { add(Stage::AfterFixedMainLoop, &[("play_camera::apply", play_camera::apply)]); }
    add(Stage::AfterFixedMainLoop, &[("scene_render::apply_camera", scene_render::apply_camera), ("scene_render::hold_camera", scene_render::hold_camera)]);

    add(Stage::Update, &[
        ("sky_render::stop_clearing", sky_render::stop_clearing),
        ("sky_render::animate_shells", sky_render::animate_shells),
        ("title_world::spawn", title_world::spawn),
        ("shrub_render::cull_system", shrub_render::cull_system),
        ("scene_render::subtitle_layer", scene_render::subtitle_layer),
        ("shadow_render::tag_camera", shadow_render::tag_camera),
        ("visibomb_view::sky", visibomb_view::sky),
        ("game_camera::print_game_camera", game_camera::print_game_camera),
        ("fly_cam::grab_cursor", fly_cam::grab_cursor),
        ("fly_cam::fly_cam", fly_cam::fly_cam),
        ("fog_state::camera_path", fog_state::camera_path),
        ("determinism::dump_frame", determinism::dump_frame),
        ("determinism::frame_screenshot", determinism::frame_screenshot),
        ("main::take_screenshot", take_screenshot),
        ("main::test_switch", test_switch),
        ("main::report_fps", report_fps),
        ("render_settings::msaa_switch", render_settings::msaa_switch),
    ]);
    if play { add(Stage::Update, &[("gameplay::respawn_probe", gameplay::respawn_probe)]); }
    add(Stage::Update, &[
        ("display::toggle_fullscreen", display::toggle_fullscreen),
        ("display::apply_mode", display::apply_mode),
        ("display::fit_present", display::fit_present),
    ]);
    if play { add(Stage::Update, &[("gameplay::debug_dump", gameplay::debug_dump)]); }
    add(Stage::Update, &[
        ("flight_render::audio_requests", flight_render::audio_requests),
        ("movie_render::target_main_camera", movie_render::target_main_camera),
    ]);
    if play {
        add(Stage::Update, &[
            ("gameplay::show_reloaded", gameplay::show_reloaded),
            ("menu_render::target_main_camera", menu_render::target_main_camera),
            ("menu_render::build_prims", menu_render::build_prims),
            ("vendor_render::draw", vendor_render::draw),
        ]);
    }
    add(Stage::Update, &[("visibomb_view::statics", visibomb_view::statics)]);
    if play { add(Stage::Update, &[("travel_render::hud_layer", travel_render::hud_layer)]); }
    if hud {
        add(Stage::Update, &[
            ("hud_render::target_main_camera", hud_render::target_main_camera),
            ("hud_render::tick_and_build", hud_render::tick_and_build),
            ("hud_render::apply_fx_cuts", hud_render::apply_fx_cuts),
        ]);
    }

    let mut post: Vec<(&'static str, SystemFn)> = vec![("display::redirect", display::redirect)];
    if play { post.push(("menu_models::update", menu_models::update)); }
    post.push(("shrub_render::update_sway", shrub_render::update_sway));
    if water { post.push(("water_render::draw", water_render::draw)); }
    post.extend_from_slice(&[
        ("display::box_views", display::box_views),
        ("sky_stars::build_meshes", sky_stars::build_meshes),
    ]);
    if play { post.push(("gameplay::upload", gameplay::upload)); }
    post.extend_from_slice(&[
        ("tesla_render::draw", tesla_render::draw),
        ("travel_render::view_layers", travel_render::view_layers),
        ("moby_attach::upload", moby_attach::upload),
        ("aa_blit::follow_option", aa_blit::follow_option),
        ("pre_shadow::tag tfrag", pre_shadow::tag::<tfrag_render::TfragMaterial>),
        ("game_camera::upload_fog", game_camera::upload_fog),
        ("pre_shadow::tag tie", pre_shadow::tag::<tie_render::TieMaterial>),
        ("render_settings::apply", render_settings::apply),
    ]);
    if water { post.push(("water_render::draw_fire_fields", water_render::draw_fire_fields)); }
    post.extend_from_slice(&[
        ("fog_state::update_fog_state", fog_state::update_fog_state),
        ("world_lights::update", world_lights::update),
        ("audio_out::run_audio", audio_out::run_audio),
        ("title_world::upload", title_world::upload),
        ("pre_shadow::tag shrub", pre_shadow::tag::<shrub_render::ShrubMaterial>),
        ("screen_tint::update", screen_tint::update),
        ("display::place_boxed", display::place_boxed),
        ("visibomb_view::overlay", visibomb_view::overlay),
    ]);
    if hud { post.push(("hud_render::apply_scaling", hud_render::apply_scaling)); }
    post.extend_from_slice(&[
        ("sky_render::follow_main_camera", sky_render::follow_main_camera),
        ("scene_render::fade_pass", scene_render::fade_pass),
    ]);
    if sea { post.push(("sea_render::draw", sea_render::draw)); }
    post.push(("afterimage_render::draw", afterimage_render::draw));
    if play { post.push(("gameplay::ratchet_visibility", gameplay::ratchet_visibility)); }
    post.extend_from_slice(&[
        ("fx_draw::draw_list1", fx_draw::draw_list1),
        ("pre_shadow::tag billboard", pre_shadow::tag::<shrub_billboard::BillboardMaterial>),
        ("travel_render::upload_actors", travel_render::upload_actors),
        ("reticle_render::draw", reticle_render::draw),
        ("reactive_render::draw", reactive_render::draw),
        ("scene_render::upload", scene_render::upload),
    ]);
    if play { post.extend_from_slice(&[("gameplay::caster_order", gameplay::caster_order), ("menu_render::menu_layer", menu_render::menu_layer)]); }
    post.extend_from_slice(&[
        ("mirror_render::follow_cheat", mirror_render::follow_cheat),
        ("moby_attach::rope_draw", moby_attach::rope_draw),
        ("walloper_render::draw", walloper_render::draw),
        ("occlusion::update_occlusion", occlusion::update_occlusion),
        ("moby_render::update_moby_occlusion", moby_render::update_moby_occlusion),
        ("tfrag_lod::update_tfrag_modes", tfrag_lod::update_tfrag_modes),
        ("tie_lod::update_tie_lods", tie_lod::update_tie_lods),
        ("particle_render::build_sprites", particle_render::build_sprites),
        ("tfrag_lod::update_lod_constants", tfrag_lod::update_lod_constants),
        ("title_world::cameras", title_world::cameras),
        ("flight_render::update_sky", flight_render::update_sky),
        ("screen_canvas::apply", screen_canvas::apply),
        ("scene_render::force_hidden", scene_render::force_hidden),
    ]);
    if play { post.push(("interact_render::hide_hero", interact_render::hide_hero)); }
    post.extend_from_slice(&[
        ("moby_spawn::force_hidden", moby_spawn::force_hidden),
        ("moby_anim::upload_palette", moby_anim::upload_palette),
    ]);
    if play { post.push(("travel_render::force_hidden", travel_render::force_hidden)); }
    post.push(("flight_render::sky_tan", flight_render::sky_tan));
    if play { post.push(("gameplay::write_visible", gameplay::write_visible)); }
    post.extend_from_slice(&[
        ("display::fly_view", display::fly_view),
        ("flight_render::draw_fx", flight_render::draw_fx),
        ("moby_attach::keep_absent_hidden", moby_attach::keep_absent_hidden),
        PROPAGATE,
    ]);
    add(Stage::PostUpdate, &post);

    // Last (rc-engine: unordered; the level swap first, then the frame's bookkeeping, the limiter last).
    add(Stage::Last, &[
        ("level_switch::apply_switch", level_switch::apply_switch),
        ("display::redirect", display::redirect),
        ("occlusion::print_stats", occlusion::print_stats),
        ("moby_render::update_extra_metal", moby_render::update_extra_metal),
        ("shadow_render::collect", shadow_render::collect),
        ("determinism::exit_before_capture", determinism::exit_before_capture),
        ("frame_log::log", frame_log::log),
        PROPAGATE,
        ("frame_pace::wait", frame_pace::wait),
    ]);
}

/// FixedUpdate: rc-engine's `GameTick` set (`gameplay::tick`) with its run conditions (crate::scene_render: a running
/// scene that stops the world, a fade hold; crate::movie_render: a blocking movie).
fn game_tick(ctx: &mut Ctx) {
    if !scene_render::tick_allowed(ctx) || !movie_render::tick_allowed(ctx) { return; }
    gameplay::tick(ctx);
}

/// Startup: the main world camera, once per process (crate::level_switch keeps it across level changes); framed on the
/// boot level's tfrags (the play camera takes over from the first tick).
fn setup_camera(ctx: &mut Ctx) {
    let level_res = ctx.res::<Level>();
    let level = &level_res.0;

    // Camera framing from the tfrag bounding spheres. They are stored in integer position units
    // (x1024 like origin + local), not world units: on Novalis the raw centroid is ~(166138, 189596, 59301)
    // while the LOD-0 vertices span (22..322, 45..388, 30..112) after /1024.
    let spheres: Vec<[f32; 4]> = level.tfrags.iter().map(|t| t.header.bsphere.map(|v| v / 1024.0)).collect();
    let n = spheres.len().max(1) as f32;
    let c = spheres.iter().fold(Vec3::ZERO, |a, s| a + Vec3::new(s[0], s[1], s[2])) / n;
    let extent = spheres.iter().map(|s| (Vec3::new(s[0], s[1], s[2]) - c).length() + s[3]).fold(0.0f32, f32::max);
    println!("bounds (game units): sphere centroid {:.1?}, extent {:.1}", c.to_array(), extent);
    let (eye, target) = match std::env::var("RC_CAM").ok().and_then(|v| parse_cam(&v)) {
        Some((e, t)) => (game_to_bevy(e), game_to_bevy(t)),
        // Up (game +Z) and back (game -Y) from the centroid, looking at it.
        None => (game_to_bevy((c + Vec3::new(0.0, -0.35 * extent, 0.25 * extent)).to_array()), game_to_bevy(c.to_array())),
    };
    let transform = Transform::from_translation(eye).looking_at(target, Vec3::Y);
    let speed = (extent / 20.0).clamp(5.0, 200.0);
    drop(level_res);
    ctx.world.spawn((
        Camera3d,
        Camera::default(),
        game_camera::game_projection(),
        render_settings::WorldCamera,
        // The UI nodes without a target camera draw on the game frame (not on the present camera).
        IsDefaultUiCamera,
        FlyCam::from_transform(&transform, speed),
        transform,
        level_switch::KeepAcrossLevels,
        Name::new("main camera"),
    ));
}

/// `ex,ey,ez,tx,ty,tz` in game coordinates.
pub fn parse_cam(s: &str) -> Option<([f32; 3], [f32; 3])> {
    let v: Vec<f32> = s.split(',').map(|x| x.trim().parse().ok()).collect::<Option<_>>()?;
    (v.len() == 6).then(|| ([v[0], v[1], v[2]], [v[3], v[4], v[5]]))
}

/// LevelStartup: the tfrags.
fn setup(ctx: &mut Ctx) {
    let level = ctx.res.get::<Level>();
    let (s, lod) = tfrag_render::spawn_tfrags(
        &mut ctx.world,
        &level.0,
        &mut ctx.res.get_mut::<Assets<Mesh>>(),
        &mut ctx.res.get_mut::<Assets<Image>>(),
        &mut ctx.res.get_mut::<Assets<tfrag_render::TfragMaterial>>(),
        &mut ctx.res.get_mut::<Assets<ShaderBuffer>>(),
    );
    drop(level);
    ctx.res.insert(lod);
    ctx.res.insert(tfrag_lod::ModesLocal::default());
    println!(
        "tfrags: {} tfrags, {} LOD-0 triangles, {} vertices, {} meshes ({} with the GS alpha-test split), {} images; mesh build {:.1} ms, asset upload {:.1} ms; vertex AABB {:.1?} .. {:.1?}",
        s.tfrags, s.triangles, s.vertices, s.meshes, s.blended_meshes, s.images, ms(s.mesh_build), ms(s.upload), s.min, s.max
    );
}

/// [`report_fps`]' local (rc-engine: `Local<Option<(Instant, u32)>>`).
#[derive(Default)]
struct FpsLocal(Option<(std::time::Instant, u32)>);

/// Update: drawn frames per second over 5 s of wall-clock time (the game clock advances by whole ticks:
/// crate::frame_pace).
fn report_fps(ctx: &mut Ctx) {
    let mut acc = ctx.res_mut::<FpsLocal>();
    let now = std::time::Instant::now();
    let (start, n) = acc.0.get_or_insert((now, 0));
    *n += 1;
    let t = (now - *start).as_secs_f32();
    if t >= 5.0 {
        println!("fps: {:.1}", *n as f32 / t);
        acc.0 = Some((now, 0));
    }
}

/// Update, `RC_METAL_SWITCH=<frame>:<level>` (test switch): at that update, the game's `ShipTravelTo(level)`
/// (crate::travel_render::request_ship_travel), the planet page's travel, so a level change can be run without the menus.
fn test_switch(ctx: &mut Ctx) {
    static SWITCH: std::sync::OnceLock<Option<(u64, i32)>> = std::sync::OnceLock::new();
    let Some((frame, dest)) = *SWITCH.get_or_init(|| {
        let v = std::env::var("RC_METAL_SWITCH").ok()?;
        let (f, l) = v.trim().split_once(':')?;
        Some((f.parse().ok()?, l.parse().ok()?))
    }) else { return };
    if ctx.res::<determinism::FrameNumber>().0 == frame { travel_render::request_ship_travel(dest); }
}

/// Update, `RC_SCREENSHOT` without `RC_SCREENSHOT_FRAME`.
fn take_screenshot(ctx: &mut Ctx) {
    let Some(mut req) = ctx.try_res_mut::<ScreenshotRequest>() else { return };
    req.frames += 1;
    if req.requested || req.frames < req.min_frames || req.start.elapsed() < req.delay { return; }
    req.requested = true;
    let path = req.path.clone();
    drop(req);
    // The game frame (crate::display), not the window: no bars, and no blank capture of an occluded window.
    let target = Some(ctx.res::<display::GameFrame>().image.clone());
    ctx.res_mut::<determinism::Captures>().0.push(determinism::CaptureRequest {
        target,
        done: Box::new(move |w, h, rgba, res| {
            match determinism::save_rgb(&path, w, h, rgba) {
                Ok(()) => println!("screenshot saved to {}", path.display()),
                Err(e) => eprintln!("cannot save screenshot to {}: {e}", path.display()),
            }
            res.get_mut::<AppExit>().0 = Some(0);
        }),
    });
}
