//! `chlorum wallpaper` — live wallpaper: a fragment shader drawn on the background layer
//! of every screen. Settings come from ~/.config/chlorum/wallpaper and are
//! picked up while running. Drawing stops while an app covers the screen
//! (the shell creates $XDG_RUNTIME_DIR/chlorum/wallpaper-paused).
//!
//! Drawing a shader live costs the graphics card all the time. So each
//! style (with its settings) is drawn once, in the background, into a video
//! of the screen's real size at 60 frames a second that loops without a
//! seam (`chlorum wallpaper-render`), kept in ~/.cache/chlorum/wallpaper.
//! From then on the wallpaper is that video, played by the graphics card
//! (mpvpaper); the live drawing is only shown until the video is ready.

use std::{
    collections::HashMap,
    path::PathBuf,
    time::{Duration, Instant, SystemTime},
};

use glow::HasContext;
use khronos_egl as egl;
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_layer, delegate_output, delegate_registry,
    output::{OutputHandler, OutputState},
    reexports::calloop::{
        timer::{TimeoutAction, Timer},
        EventLoop,
    },
    reexports::calloop_wayland_source::WaylandSource,
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::{
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
        WaylandSurface,
    },
};
use wayland_client::{
    delegate_noop,
    globals::registry_queue_init,
    protocol::{wl_output, wl_surface},
    Connection, Proxy, QueueHandle,
};
use wayland_protocols::wp::viewporter::client::{wp_viewport::WpViewport, wp_viewporter::WpViewporter};

/// Drawn at the screen's full size: the shaders read their random numbers
/// from a texture made once at start, so full quality is cheap.
const RENDER_SCALE: f64 = 1.0;
/// Frames per second, at most. Frames follow the screen's refresh, so a
/// 60 Hz screen gets every refresh and a 144 Hz one an even 72.
const FPS: f64 = 60.0;
/// The clock goes back to 0 past this many (scaled) seconds while hidden,
/// and in any case past `CLOCK_MAX`, so it never loses precision.
const CLOCK_WRAP: f64 = 2048.0;
const CLOCK_MAX: f64 = 16384.0;

const COMMON: &str = include_str!("shaders/common.glsl");
const VERTEX: &str = "attribute vec2 p; void main(){ gl_Position = vec4(p, 0.0, 1.0); }";

fn shader_source(style: &str) -> Option<&'static str> {
    Some(match style {
        "lava" => include_str!("shaders/lava.glsl"),
        "gas" => include_str!("shaders/gas.glsl"),
        "beams" => include_str!("shaders/beams.glsl"),
        "particles" => include_str!("shaders/particles.glsl"),
        "shells" => include_str!("shaders/shells.glsl"),
        _ => return None,
    })
}

#[derive(Clone, PartialEq)]
struct Settings {
    style: String,
    speed: f32,
    brightness: f32,
    amount: f32,
    color: [f32; 3],
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            style: "lava".into(),
            speed: 0.4,
            brightness: 0.7,
            amount: 6.0,
            color: [1.0, 0.839, 0.039],
        }
    }
}

fn config_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config"));
    base.join("chlorum/wallpaper")
}

fn paused_path() -> PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|| "/tmp".into());
    base.join("chlorum/wallpaper-paused")
}

fn parse_color(s: &str) -> Option<[f32; 3]> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(s, 16).ok()?;
    Some([
        ((v >> 16) & 255) as f32 / 255.0,
        ((v >> 8) & 255) as f32 / 255.0,
        (v & 255) as f32 / 255.0,
    ])
}

fn load_settings() -> Settings {
    let mut s = Settings::default();
    let Ok(text) = std::fs::read_to_string(config_path()) else { return s };
    for line in text.lines() {
        let Some((k, v)) = line.split_once('=') else { continue };
        let v = v.trim();
        match k.trim() {
            "style" => s.style = v.to_string(),
            "speed" => s.speed = v.parse().unwrap_or(s.speed),
            "brightness" => s.brightness = v.parse().unwrap_or(s.brightness),
            "amount" => s.amount = v.parse().unwrap_or(s.amount),
            "color" => s.color = parse_color(v).unwrap_or(s.color),
            _ => {}
        }
    }
    s.speed = s.speed.clamp(0.0, 1.0);
    s.brightness = s.brightness.clamp(0.05, 1.0);
    s.amount = s.amount.clamp(1.0, 10.0);
    s
}

struct Gl {
    egl: egl::Instance<egl::Static>,
    display: egl::Display,
    config: egl::Config,
    context: egl::Context,
    gl: glow::Context,
    program: Option<glow::Program>,
    style: String,
}

/// 256 x 256 texels of four random bytes each, the same every time.
fn random_texels() -> Vec<u8> {
    let mut x: u32 = 0x9e37_79b9;
    (0..256 * 256 * 4)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            (x >> 24) as u8
        })
        .collect()
}

impl Gl {
    fn new(conn: &Connection) -> Gl {
        let egl = egl::Instance::new(egl::Static);
        let display = unsafe { egl.get_display(conn.display().id().as_ptr() as egl::NativeDisplayType) }
            .expect("no EGL display");
        egl.initialize(display).expect("EGL init failed");
        let attrs = [
            egl::RED_SIZE, 8, egl::GREEN_SIZE, 8, egl::BLUE_SIZE, 8,
            egl::SURFACE_TYPE, egl::WINDOW_BIT,
            egl::RENDERABLE_TYPE, egl::OPENGL_ES2_BIT,
            egl::NONE,
        ];
        let config = egl.choose_first_config(display, &attrs).ok().flatten().expect("no EGL config");
        egl.bind_api(egl::OPENGL_ES_API).expect("no GLES");
        let context = egl
            .create_context(display, config, None, &[egl::CONTEXT_CLIENT_VERSION, 2, egl::NONE])
            .expect("no GLES context");
        egl.make_current(display, None, None, Some(context)).ok();
        let gl = unsafe {
            glow::Context::from_loader_function(|s| {
                egl.get_proc_address(s).map_or(std::ptr::null(), |p| p as *const _)
            })
        };
        unsafe {
            let buf = gl.create_buffer().unwrap();
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(buf));
            let tri: [f32; 6] = [-1.0, -1.0, 3.0, -1.0, -1.0, 3.0];
            let bytes = std::slice::from_raw_parts(tri.as_ptr() as *const u8, 24);
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes, glow::STATIC_DRAW);
            // the random numbers, computed once: unit 0 unfiltered, unit 1 blended
            let texels = random_texels();
            for (unit, filter) in [(glow::TEXTURE0, glow::NEAREST), (glow::TEXTURE1, glow::LINEAR)] {
                gl.active_texture(unit);
                let tex = gl.create_texture().unwrap();
                gl.bind_texture(glow::TEXTURE_2D, Some(tex));
                gl.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
                gl.tex_image_2d(
                    glow::TEXTURE_2D, 0, glow::RGBA as i32, 256, 256, 0, glow::RGBA, glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(Some(&texels)),
                );
                // no mipmaps: a mipmap filter here would leave the texture black
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, filter as i32);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, filter as i32);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::REPEAT as i32);
                gl.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::REPEAT as i32);
            }
        }
        Gl { egl, display, config, context, gl, program: None, style: String::new() }
    }

    /// Which graphics the drawing runs on ("NVIDIA GeForce RTX 4070/PCIe/SSE2").
    fn renderer(&self) -> String {
        unsafe { self.gl.get_parameter_string(glow::RENDERER) }
    }

    fn use_style(&mut self, style: &str) {
        if self.style == style {
            return;
        }
        self.style = style.to_string();
        unsafe {
            if let Some(p) = self.program.take() {
                self.gl.delete_program(p);
            }
            let Some(frag) = shader_source(style) else { return };
            let gl = &self.gl;
            let program = gl.create_program().unwrap();
            for (kind, src) in [
                (glow::VERTEX_SHADER, VERTEX.to_string()),
                (glow::FRAGMENT_SHADER, format!("{COMMON}\n{frag}")),
            ] {
                let sh = gl.create_shader(kind).unwrap();
                gl.shader_source(sh, &src);
                gl.compile_shader(sh);
                if !gl.get_shader_compile_status(sh) {
                    eprintln!("chlorum-wallpaper: {style}: {}", gl.get_shader_info_log(sh));
                    return;
                }
                gl.attach_shader(program, sh);
            }
            gl.bind_attrib_location(program, 0, "p");
            gl.link_program(program);
            if !gl.get_program_link_status(program) {
                eprintln!("chlorum-wallpaper: {style}: {}", gl.get_program_info_log(program));
                return;
            }
            gl.use_program(Some(program));
            gl.uniform_1_i32(gl.get_uniform_location(program, "rnd").as_ref(), 0);
            gl.uniform_1_i32(gl.get_uniform_location(program, "smo").as_ref(), 1);
            self.program = Some(program);
        }
    }
}

struct Screen {
    layer: LayerSurface,
    output: wl_output::WlOutput,
    size: (u32, u32),
    egl_window: Option<wayland_egl::WlEglSurface>,
    egl_surface: Option<egl::Surface>,
    waiting: bool, // a frame callback is pending
    viewport: Option<WpViewport>,
    /// the size actually drawn (smaller than the screen when scaled)
    drawn: (u32, u32),
    last_draw: Option<Instant>,
}

struct App {
    registry: RegistryState,
    outputs: OutputState,
    compositor: CompositorState,
    layer_shell: LayerShell,
    gl: Gl,
    screens: HashMap<u32, Screen>, // by wl_output protocol id
    settings: Settings,
    settings_mtime: Option<SystemTime>,
    paused: bool,
    clock: f64,
    last: Instant,
    /// frames drawn and seconds spent running, for the log
    frames: u64,
    active: f64,
    log_at: f64,
    viewporter: Option<WpViewporter>,
}

impl App {
    fn reload(&mut self) {
        let mtime = std::fs::metadata(config_path()).and_then(|m| m.modified()).ok();
        if mtime != self.settings_mtime {
            self.settings_mtime = mtime;
            self.settings = load_settings();
        }
        self.paused = paused_path().exists() || shader_source(&self.settings.style).is_none();
    }

    fn draw(&mut self, id: u32, qh: &QueueHandle<Self>) {
        let now = Instant::now();
        let dt = now.duration_since(self.last).as_secs_f64().min(0.1);
        self.last = now;
        // 0% is almost still, 100% is lively.
        self.clock += dt * (0.08 + 1.6 * self.settings.speed as f64);
        if self.clock > CLOCK_MAX {
            self.clock = 0.0;
        }
        self.frames += 1;
        let style = self.settings.style.clone();
        self.gl.use_style(&style);
        let Some(screen) = self.screens.get_mut(&id) else { return };
        let Some(surface) = screen.egl_surface else { return };
        let g = &self.gl;
        g.egl.make_current(g.display, Some(surface), Some(surface), Some(g.context)).ok();
        // frames are paced by the compositor's frame callbacks; never block here
        g.egl.swap_interval(g.display, 0).ok();
        let (w, h) = screen.drawn;
        screen.last_draw = Some(now);
        unsafe {
            let gl = &g.gl;
            gl.viewport(0, 0, w as i32, h as i32);
            gl.clear_color(0.0, 0.0, 0.0, 1.0);
            gl.clear(glow::COLOR_BUFFER_BIT);
            if let Some(p) = g.program {
                gl.use_program(Some(p));
                let s = &self.settings;
                gl.uniform_1_f32(gl.get_uniform_location(p, "t").as_ref(), self.clock as f32);
                gl.uniform_2_f32(gl.get_uniform_location(p, "res").as_ref(), w as f32, h as f32);
                gl.uniform_1_f32(gl.get_uniform_location(p, "bright").as_ref(), s.brightness);
                gl.uniform_1_f32(gl.get_uniform_location(p, "amount").as_ref(), s.amount);
                gl.uniform_3_f32(gl.get_uniform_location(p, "col").as_ref(), s.color[0], s.color[1], s.color[2]);
                gl.enable_vertex_attrib_array(0);
                gl.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 0, 0);
                gl.draw_arrays(glow::TRIANGLES, 0, 3);
            }
        }
        let wl = screen.layer.wl_surface();
        if !self.paused {
            wl.frame(qh, wl.clone());
            screen.waiting = true;
        } else {
            screen.waiting = false;
        }
        g.egl.swap_buffers(g.display, surface).ok();
    }

    fn id_of(&self, surface: &wl_surface::WlSurface) -> Option<u32> {
        self.screens.iter().find(|(_, s)| s.layer.wl_surface() == surface).map(|(k, _)| *k)
    }

    fn wake(&mut self, qh: &QueueHandle<Self>) {
        let idle: Vec<u32> = self
            .screens
            .iter()
            .filter(|(_, s)| !s.waiting && s.egl_surface.is_some())
            .map(|(k, _)| *k)
            .collect();
        self.last = Instant::now();
        for id in idle {
            self.draw(id, qh);
        }
    }
}

impl CompositorHandler for App {
    fn scale_factor_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: i32) {}
    fn transform_changed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: wl_output::Transform) {}
    fn frame(&mut self, _: &Connection, qh: &QueueHandle<Self>, surface: &wl_surface::WlSurface, _: u32) {
        let Some(id) = self.id_of(surface) else { return };
        let since = self.screens.get(&id).and_then(|s| s.last_draw).map(|t| t.elapsed().as_secs_f64()).unwrap_or(1.0);
        if let Some(s) = self.screens.get_mut(&id) {
            s.waiting = false;
        }
        if self.paused {
            return;
        }
        // Too soon for the next frame: skip this refresh and wait for the
        // next one, so frames stay in step with the screen.
        if since < 0.75 / FPS {
            if let Some(s) = self.screens.get_mut(&id) {
                let wl = s.layer.wl_surface();
                wl.frame(qh, wl.clone());
                wl.commit();
                s.waiting = true;
            }
            return;
        }
        self.draw(id, qh);
    }
    fn surface_enter(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
    fn surface_leave(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: &wl_output::WlOutput) {}
}

impl OutputHandler for App {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.outputs
    }
    fn new_output(&mut self, _: &Connection, qh: &QueueHandle<Self>, output: wl_output::WlOutput) {
        let surface = self.compositor.create_surface(qh);
        let layer = self.layer_shell.create_layer_surface(
            qh,
            surface,
            Layer::Background,
            Some("chlorum-wallpaper"),
            Some(&output),
        );
        layer.set_anchor(Anchor::TOP | Anchor::BOTTOM | Anchor::LEFT | Anchor::RIGHT);
        layer.set_exclusive_zone(-1);
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        let viewport = self.viewporter.as_ref().map(|v| v.get_viewport(layer.wl_surface(), qh, ()));
        layer.commit();
        self.screens.insert(
            output.id().protocol_id(),
            Screen { layer, output: output.clone(), size: (0, 0), egl_window: None, egl_surface: None, waiting: false, viewport, drawn: (0, 0), last_draw: None },
        );
    }
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, output: wl_output::WlOutput) {
        if let Some(s) = self.screens.remove(&output.id().protocol_id()) {
            if let Some(es) = s.egl_surface {
                self.gl.egl.destroy_surface(self.gl.display, es).ok();
            }
        }
    }
}

impl LayerShellHandler for App {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        self.screens.retain(|_, s| &s.layer != layer);
    }
    fn configure(
        &mut self,
        _: &Connection,
        qh: &QueueHandle<Self>,
        layer: &LayerSurface,
        configure: LayerSurfaceConfigure,
        _: u32,
    ) {
        let Some(id) = self.id_of(layer.wl_surface()) else { return };
        let (w, h) = (configure.new_size.0.max(1), configure.new_size.1.max(1));
        // the screen's real pixels: a 4K screen at 200% is 1920 x 1080 to
        // the layout but 3840 x 2160 to look at
        let sf = self.screens.get(&id).and_then(|s| self.outputs.info(&s.output)).map(|i| i.scale_factor.max(1)).unwrap_or(1) as f64;
        let g = &self.gl;
        let screen = self.screens.get_mut(&id).unwrap();
        if screen.size != (w, h) || screen.egl_window.is_none() || screen.drawn.0 != ((w as f64) * sf * RENDER_SCALE).round() as u32 {
            screen.size = (w, h);
            let (dw, dh) = match &screen.viewport {
                Some(vp) => {
                    vp.set_destination(w as i32, h as i32);
                    (((w as f64) * sf * RENDER_SCALE).round().max(1.0) as u32, ((h as f64) * sf * RENDER_SCALE).round().max(1.0) as u32)
                }
                None => (w, h),
            };
            screen.drawn = (dw, dh);
            if let Some(win) = &screen.egl_window {
                win.resize(dw as i32, dh as i32, 0, 0);
            } else {
                let win = wayland_egl::WlEglSurface::new(layer.wl_surface().id(), dw as i32, dh as i32)
                    .expect("wl_egl_window");
                let es = unsafe {
                    g.egl.create_window_surface(g.display, g.config, win.ptr() as egl::NativeWindowType, None)
                }
                .expect("EGL surface");
                screen.egl_window = Some(win);
                screen.egl_surface = Some(es);
            }
        }
        if !screen.waiting {
            self.draw(id, qh);
        }
    }
}

impl ProvidesRegistryState for App {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry
    }
    registry_handlers![OutputState];
}

delegate_compositor!(App);
delegate_output!(App);
delegate_layer!(App);
delegate_registry!(App);
delegate_noop!(App: ignore WpViewporter);
delegate_noop!(App: ignore WpViewport);

pub fn run() {
    // the video, when it is made for these settings and this screen
    let settings = load_settings();
    let (w, h) = screen_size();
    // the player did not work this time: only the live drawing
    let live_only = std::env::var_os("CHLORUM_WALLPAPER_LIVE").is_some();
    if shader_source(&settings.style).is_some() && !live_only {
        let video = video_path(&settings, w, h);
        if video.exists() {
            play(&video, &settings);
        }
        start_render(w, h);
    }
    let conn = Connection::connect_to_env().expect("not in a Wayland session");
    let (globals, queue) = registry_queue_init::<App>(&conn).expect("Wayland registry");
    let qh = queue.handle();
    let mut event_loop: EventLoop<App> = EventLoop::try_new().unwrap();
    WaylandSource::new(conn.clone(), queue).insert(event_loop.handle()).unwrap();

    let mut app = App {
        registry: RegistryState::new(&globals),
        outputs: OutputState::new(&globals, &qh),
        compositor: CompositorState::bind(&globals, &qh).expect("wl_compositor"),
        layer_shell: LayerShell::bind(&globals, &qh).expect("layer shell"),
        gl: Gl::new(&conn),
        screens: HashMap::new(),
        settings: load_settings(),
        settings_mtime: None,
        paused: false,
        clock: 0.0,
        last: Instant::now(),
        frames: 0,
        active: 0.0,
        log_at: 10.0,
        viewporter: globals.bind(&qh, 1..=1, ()).ok(),
    };
    app.reload();
    eprintln!("chlorum-wallpaper: drawing live with {}", app.gl.renderer());
    let (vw, vh) = (w, h);
    let mut rendering_for = app.settings.clone();

    // Twice a second: notice new settings and pause or resume.
    let qh2 = qh.clone();
    event_loop
        .handle()
        .insert_source(Timer::from_duration(Duration::from_millis(500)), move |_, _, app| {
            let was = app.paused;
            app.reload();
            // the video is ready: from now on it is the wallpaper
            if !live_only && shader_source(&app.settings.style).is_some() && video_path(&app.settings, vw, vh).exists() {
                restart();
            }
            // new settings: their video is made (the old one is stopped by itself)
            if !live_only && app.settings != rendering_for {
                rendering_for = app.settings.clone();
                if shader_source(&app.settings.style).is_some() {
                    start_render(vw, vh);
                }
            }
            if app.paused {
                // hidden: a good moment to restart the clock unseen
                if app.clock > CLOCK_WRAP {
                    app.clock = 0.0;
                }
            } else {
                app.active += 0.5;
                // how smooth it runs, in the journal: after 10 s, then every 10 min
                if app.active >= app.log_at {
                    let sizes: Vec<String> = app.screens.values().map(|s| format!("{}x{}", s.drawn.0, s.drawn.1)).collect();
                    let per_screen = app.frames as f64 / app.active / app.screens.len().max(1) as f64;
                    eprintln!("chlorum-wallpaper: {} at {:.1} fps on {}", app.settings.style, per_screen, sizes.join(", "));
                    app.frames = 0;
                    app.active = 0.0;
                    app.log_at = 600.0;
                }
            }
            if was && !app.paused {
                app.wake(&qh2);
            } else if !app.paused {
                // style or colour changed while running: frames pick it up
            } else {
                // paused: draw one still frame so a style change still shows
                let idle: Vec<u32> = app.screens.keys().copied().collect();
                if app.gl.style != app.settings.style {
                    for id in idle {
                        app.draw(id, &qh2);
                    }
                }
            }
            TimeoutAction::ToDuration(Duration::from_millis(500))
        })
        .unwrap();

    loop {
        event_loop.dispatch(None, &mut app).unwrap();
    }
}


// ------------------------------------------------------------ the video

/// Seconds in the loop, and of the blend from its end into its start.
const LOOP: u32 = 30;
const BLEND: u32 = 2;

fn cache_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".cache"));
    base.join("chlorum/wallpaper")
}

fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|| "/tmp".into()).join("chlorum")
}

/// The video of these settings at this size.
fn video_path(s: &Settings, w: u32, h: u32) -> PathBuf {
    use std::hash::{Hash, Hasher};
    let mut hs = std::collections::hash_map::DefaultHasher::new();
    s.style.hash(&mut hs);
    for v in [s.speed, s.brightness, s.amount, s.color[0], s.color[1], s.color[2]] {
        ((v * 1000.0).round() as i64).hash(&mut hs);
    }
    // a new version of a shader makes a new video
    shader_source(&s.style).unwrap_or("").hash(&mut hs);
    COMMON.hash(&mut hs);
    cache_dir().join(format!("{}-{:012x}-{w}x{h}.mp4", s.style, hs.finish() & 0xffff_ffff_ffff))
}

/// The biggest screen's real size (what the video is made at).
fn screen_size() -> (u32, u32) {
    let Ok(mut sock) = niri_ipc::socket::Socket::connect() else { return (3840, 2160) };
    let Ok(Ok(niri_ipc::Response::Outputs(outs))) = sock.send(niri_ipc::Request::Outputs) else { return (3840, 2160) };
    outs.values()
        .filter_map(|o| {
            let m = o.modes.get(o.current_mode?)?;
            // turned a quarter: drawn upright, shown turned
            let turned = matches!(o.logical.as_ref().map(|l| l.transform), Some(niri_ipc::Transform::_90 | niri_ipc::Transform::_270 | niri_ipc::Transform::Flipped90 | niri_ipc::Transform::Flipped270));
            Some(if turned { (m.height as u32, m.width as u32) } else { (m.width as u32, m.height as u32) })
        })
        .max_by_key(|(w, h)| w * h)
        .unwrap_or((3840, 2160))
}

/// Starts making the video in the background, if it is not being made.
fn start_render(w: u32, h: u32) {
    let Ok(me) = std::env::current_exe() else { return };
    let _ = std::process::Command::new(me)
        .args(["wallpaper-render", &w.to_string(), &h.to_string()])
        .stdin(std::process::Stdio::null())
        .spawn();
}

/// Starts the wallpaper again (now as the video).
fn restart() -> ! {
    use std::os::unix::process::CommandExt;
    let me = std::env::current_exe().unwrap_or_else(|_| "chlorum".into());
    let e = std::process::Command::new(me).arg("wallpaper").exec();
    eprintln!("chlorum-wallpaper: {e}");
    std::process::exit(1)
}

/// Shows the video until the settings change; pauses it while an app
/// covers the screen.
fn play(video: &std::path::Path, settings: &Settings) -> ! {
    let sock = runtime_dir().join("wallpaper-mpv.sock");
    let _ = std::fs::create_dir_all(runtime_dir());
    let _ = std::fs::remove_file(&sock);
    let opts = format!(
        "no-audio loop-file=inf hwdec=auto-safe panscan=1.0 keepaspect=yes input-ipc-server={} osc=no osd-level=0 really-quiet",
        sock.display()
    );
    let mut child = match std::process::Command::new("mpvpaper").args(["-o", &opts, "*"]).arg(video).stdin(std::process::Stdio::null()).spawn() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("chlorum-wallpaper: no mpvpaper ({e}); drawing live");
            live_after_all();
        }
    };
    eprintln!("chlorum-wallpaper: playing {}", video.display());
    let mut paused = false;
    loop {
        std::thread::sleep(Duration::from_millis(500));
        if let Ok(Some(st)) = child.try_wait() {
            eprintln!("chlorum-wallpaper: the player stopped ({st}); drawing live");
            live_after_all();
        }
        // new settings: the video of those (or the live drawing meanwhile)
        if load_settings() != *settings {
            let _ = child.kill();
            let _ = child.wait();
            restart();
        }
        let now = paused_path().exists();
        if now != paused {
            paused = now;
            if let Ok(mut s) = std::os::unix::net::UnixStream::connect(&sock) {
                use std::io::Write;
                let _ = writeln!(s, "{{\"command\":[\"set_property\",\"pause\",{}]}}", if paused { "true" } else { "false" });
            }
        }
    }
}

/// The video could not be played: live drawing, and no new tries this run.
fn live_after_all() -> ! {
    use std::os::unix::process::CommandExt;
    let me = std::env::current_exe().unwrap_or_else(|_| "chlorum".into());
    let e = std::process::Command::new(me).arg("wallpaper").env("CHLORUM_WALLPAPER_LIVE", "1").exec();
    eprintln!("chlorum-wallpaper: {e}");
    std::process::exit(1)
}

/// `chlorum wallpaper-render W H`: draws the wallpaper's loop on the
/// graphics card, frame by frame, into the video (two runs of ffmpeg: the
/// frames, then the end blended into the start). Progress is in
/// $XDG_RUNTIME_DIR/chlorum/wallpaper-render ("37").
pub fn render_cli(args: &[String]) -> Result<(), String> {
    let w: u32 = args.first().and_then(|a| a.parse().ok()).unwrap_or(3840);
    let h: u32 = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(2160);
    // one at a time: a later one waits, then makes the video of whatever
    // the settings are by then (a slider moved many times makes one)
    let _ = std::fs::create_dir_all(runtime_dir());
    let lock = runtime_dir().join("wallpaper-render.lock");
    let lock_file = std::fs::OpenOptions::new().create(true).write(true).truncate(false).open(&lock).map_err(|e| e.to_string())?;
    if unsafe { libc::flock(std::os::fd::AsRawFd::as_raw_fd(&lock_file), libc::LOCK_EX) } != 0 {
        return Ok(());
    }
    loop {
        match render_once(w, h) {
            Err(e) if e.contains("the settings changed") => continue,
            r => return r,
        }
    }
}

fn render_once(w: u32, h: u32) -> Result<(), String> {
    let settings = load_settings();
    if shader_source(&settings.style).is_none() {
        return Ok(());
    }
    let out = video_path(&settings, w, h);
    if out.exists() {
        return Ok(());
    }
    let dir = cache_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    // old videos go (each is a few hundred megabytes)
    if let Ok(rd) = std::fs::read_dir(&dir) {
        for e in rd.flatten() {
            if e.path() != out {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    let progress = runtime_dir().join("wallpaper-render");
    let say = |pct: u32| {
        let _ = std::fs::write(&progress, pct.to_string());
    };
    say(0);
    let frames_path = dir.join("frames.mkv");
    let r = (|| -> Result<(), String> {
        let conn = Connection::connect_to_env().map_err(|e| e.to_string())?;
        let mut gl = Gl::new(&conn);
        let renderer = gl.renderer();
        eprintln!("chlorum-wallpaper: making the {w}x{h} video of {} with {renderer}", settings.style);
        // without a graphics card (software drawing) a 4K video would take
        // hours: then it is made at 1080p and scaled up when shown
        let soft = renderer.to_lowercase().contains("llvmpipe") || renderer.to_lowercase().contains("softpipe");
        let (w, h) = if soft { (w.min(1920), h.min(1080)) } else { (w, h) };
        gl.use_style(&settings.style);
        let Some(program) = gl.program else { return Err("the shader did not compile".into()) };
        let g = &gl.gl;
        let fps = 60u32;
        let total = (LOOP + BLEND) * fps;
        unsafe {
            let tex = g.create_texture()?;
            g.active_texture(glow::TEXTURE2);
            g.bind_texture(glow::TEXTURE_2D, Some(tex));
            g.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA as i32, w as i32, h as i32, 0, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(None));
            let fbo = g.create_framebuffer()?;
            g.bind_framebuffer(glow::FRAMEBUFFER, Some(fbo));
            g.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(tex), 0);
            if g.check_framebuffer_status(glow::FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE {
                return Err("no offscreen frame".into());
            }
        }
        let nvenc = nvenc_works();
        let mut ff = ffmpeg_frames(w, h, fps, &frames_path, nvenc)?;
        let mut stdin = ff.stdin.take().ok_or("no ffmpeg input")?;
        let mut buf = vec![0u8; (w * h * 4) as usize];
        let rate = 0.08 + 1.6 * settings.speed as f64;
        for k in 0..total {
            let clock = (k as f64 / fps as f64) * rate;
            unsafe {
                g.viewport(0, 0, w as i32, h as i32);
                g.use_program(Some(program));
                g.uniform_1_f32(g.get_uniform_location(program, "t").as_ref(), clock as f32);
                g.uniform_2_f32(g.get_uniform_location(program, "res").as_ref(), w as f32, h as f32);
                g.uniform_1_f32(g.get_uniform_location(program, "bright").as_ref(), settings.brightness);
                g.uniform_1_f32(g.get_uniform_location(program, "amount").as_ref(), settings.amount);
                g.uniform_3_f32(g.get_uniform_location(program, "col").as_ref(), settings.color[0], settings.color[1], settings.color[2]);
                g.enable_vertex_attrib_array(0);
                g.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 0, 0);
                g.draw_arrays(glow::TRIANGLES, 0, 3);
                g.read_pixels(0, 0, w as i32, h as i32, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelPackData::Slice(Some(&mut buf)));
            }
            use std::io::Write;
            stdin.write_all(&buf).map_err(|e| format!("ffmpeg stopped taking frames: {e}"))?;
            if k % 30 == 0 {
                say(k * 80 / total);
            }
            // settings changed meanwhile: this video is not wanted any more
            if k % 120 == 0 && load_settings() != settings {
                return Err("the settings changed".into());
            }
        }
        drop(stdin);
        let st = ff.wait().map_err(|e| e.to_string())?;
        if !st.success() {
            return Err("ffmpeg could not make the frames".into());
        }
        say(85);
        // the loop: from BLEND seconds in to the end, the last BLEND
        // seconds blended into the first ones, so its end is its start
        let tmp = out.with_extension("part.mp4");
        let filter = format!(
            "[0:v]trim=start={b}:end={e},setpts=PTS-STARTPTS[a];[0:v]trim=start=0:end={b},setpts=PTS-STARTPTS[s];[a][s]xfade=transition=fade:duration={b}:offset={o},format=yuv420p[v]",
            b = BLEND,
            e = LOOP + BLEND,
            o = LOOP - BLEND
        );
        let ok = (nvenc && encode(&frames_path, &filter, &tmp, true)) || encode(&frames_path, &filter, &tmp, false);
        let _ = std::fs::remove_file(&frames_path);
        if !ok {
            let _ = std::fs::remove_file(&tmp);
            return Err("ffmpeg could not make the loop".into());
        }
        std::fs::rename(&tmp, &out).map_err(|e| e.to_string())?;
        Ok(())
    })();
    let _ = std::fs::remove_file(&progress);
    let _ = std::fs::remove_file(&frames_path);
    match r {
        Ok(()) => {
            eprintln!("chlorum-wallpaper: the video is ready: {}", out.display());
            Ok(())
        }
        Err(e) => Err(format!("wallpaper video: {e}")),
    }
}

/// Can ffmpeg use the graphics card's encoder (NVIDIA)? One small test
/// frame tells.
fn nvenc_works() -> bool {
    std::process::Command::new("ffmpeg")
        .args(["-nostdin", "-loglevel", "quiet", "-f", "lavfi", "-i", "color=c=black:s=256x256", "-frames:v", "1", "-c:v", "hevc_nvenc", "-f", "null", "-"])
        .stdin(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// ffmpeg taking raw frames: nearly lossless, fast (the graphics card's
/// encoder when there is one).
fn ffmpeg_frames(w: u32, h: u32, fps: u32, out: &std::path::Path, nvenc: bool) -> Result<std::process::Child, String> {
    let size = format!("{w}x{h}");
    let rate = fps.to_string();
    let mut c = std::process::Command::new("ffmpeg");
    c.args(["-nostdin", "-loglevel", "error", "-y", "-f", "rawvideo", "-pix_fmt", "rgba", "-s", &size, "-r", &rate, "-i", "-", "-vf", "vflip"]);
    if nvenc {
        c.args(["-c:v", "hevc_nvenc", "-preset", "p4", "-rc", "constqp", "-qp", "12"]);
    } else {
        c.args(["-c:v", "libx264", "-preset", "ultrafast", "-crf", "10"]);
    }
    c.args(["-pix_fmt", "yuv420p"]).arg(out).stdin(std::process::Stdio::piped()).stderr(std::process::Stdio::inherit()).spawn().map_err(|e| format!("no ffmpeg: {e}"))
}

fn encode(input: &std::path::Path, filter: &str, out: &std::path::Path, nvenc: bool) -> bool {
    let mut c = std::process::Command::new("ffmpeg");
    c.args(["-nostdin", "-loglevel", "error", "-y", "-i"]).arg(input).args(["-filter_complex", filter, "-map", "[v]", "-an"]);
    if nvenc {
        c.args(["-c:v", "hevc_nvenc", "-preset", "p6", "-rc", "vbr", "-cq", "20", "-tag:v", "hvc1"]);
    } else {
        c.args(["-c:v", "libx264", "-preset", "medium", "-crf", "20"]);
    }
    c.args(["-movflags", "+faststart"]).arg(out).stdin(std::process::Stdio::null()).status().map(|s| s.success()).unwrap_or(false)
}
