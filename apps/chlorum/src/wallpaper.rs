//! `chlorum wallpaper` — live wallpaper: a fragment shader drawn on the background layer
//! of every screen. Settings come from ~/.config/chlorum/wallpaper and are
//! picked up while running. Drawing stops while an app covers the screen
//! (the shell creates $XDG_RUNTIME_DIR/chlorum/wallpaper-paused).

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
            Screen { layer, size: (0, 0), egl_window: None, egl_surface: None, waiting: false, viewport, drawn: (0, 0), last_draw: None },
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
        let g = &self.gl;
        let screen = self.screens.get_mut(&id).unwrap();
        if screen.size != (w, h) || screen.egl_window.is_none() {
            screen.size = (w, h);
            // drawn smaller, shown full size
            let (dw, dh) = match &screen.viewport {
                Some(vp) => {
                    vp.set_destination(w as i32, h as i32);
                    (((w as f64) * RENDER_SCALE).round().max(1.0) as u32, ((h as f64) * RENDER_SCALE).round().max(1.0) as u32)
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

    // Twice a second: notice new settings and pause or resume.
    let qh2 = qh.clone();
    event_loop
        .handle()
        .insert_source(Timer::from_duration(Duration::from_millis(500)), move |_, _, app| {
            let was = app.paused;
            app.reload();
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
