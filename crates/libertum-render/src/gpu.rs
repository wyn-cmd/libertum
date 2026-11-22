//! GPU page rendering with vello (PLAN §4.5). Enabled by the `gpu` feature.
//!
//! A page is encoded once into a scale-independent [`vello::Scene`] (point units) from the same
//! parley glyph runs the CPU renderer uses, cached by `(page, content hash)`. Rendering at a
//! given scale appends the cached scene under a scale transform into a root scene and runs
//! vello's compute pipeline into an `Rgba8Unorm` storage texture.

use std::collections::{HashMap, VecDeque};
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex};

use libertum_layout::parley::PositionedLayoutItem;
use libertum_layout::{Decoration, LEADER_PITCH_PT, LayoutState, ParagraphLayout};
use tiny_skia::Pixmap;
use vello::kurbo::{Affine, Rect};
use vello::peniko::{Blob, Color, Fill, ImageAlphaType, ImageData, ImageFormat};
use vello::wgpu;
use vello::{AaConfig, AaSupport, Glyph, RenderParams, Renderer, RendererOptions, Scene};

use crate::backend::{PageBackend, PageOutput};

mod caps;
pub use caps::{
    GatePolicy, GpuCaps, GpuReport, GpuUnavailable, REQUIRED_DOWNLEVEL, check_caps,
    page_view_format, probe, probe_all, probe_default, probe_with, required,
};

/// Texture format vello renders into (it requires `Rgba8Unorm` + `STORAGE_BINDING`).
pub const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

/// Errors from GPU setup or rendering.
#[derive(Debug)]
pub enum GpuError {
    NoAdapter(String),
    Device(String),
    Renderer(String),
    Readback(String),
}

impl std::fmt::Display for GpuError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoAdapter(e) => write!(f, "no suitable GPU adapter: {e}"),
            Self::Device(e) => write!(f, "GPU device error: {e}"),
            Self::Renderer(e) => write!(f, "vello renderer error: {e}"),
            Self::Readback(e) => write!(f, "GPU readback error: {e}"),
        }
    }
}

impl std::error::Error for GpuError {}

/// Shared log of wgpu errors that no error scope caught (see [`install_nonfatal_error_handler`]).
#[derive(Clone, Default)]
pub struct UncapturedErrors(Arc<Mutex<Vec<String>>>);

impl UncapturedErrors {
    /// Takes and clears the recorded messages.
    pub fn take(&self) -> Vec<String> {
        self.0
            .lock()
            .map(|mut v| std::mem::take(&mut *v))
            .unwrap_or_default()
    }
}

/// Makes wgpu report uncaptured errors into `log` instead of panicking (wgpu 27's default is
/// `panic!`, see `wgpu-27.0.1/src/backend/wgpu_core.rs` `default_error_handler`).
///
/// Call this only on a device you own. Iced owns the device in the app, and this replaces the
/// handler for *every* user of that device, so the app decides whether to call it (for example
/// from `Pipeline::new`, accepting that Iced's own errors are then logged, not fatal). The
/// renderer in this module never relies on it: it uses error scopes around its own work.
pub fn install_nonfatal_error_handler(device: &wgpu::Device, log: &UncapturedErrors) {
    let sink = log.clone();
    device.on_uncaptured_error(Arc::new(move |e: wgpu::Error| {
        let msg = e.to_string();
        if let Ok(mut v) = sink.0.lock()
            && v.len() < 64
        {
            v.push(msg);
        }
    }));
}

/// A headless wgpu device for tests, benchmarks and tools. In the app, use Iced's device instead.
///
/// Uncaptured wgpu errors are recorded in [`HeadlessGpu::uncaptured`], never fatal.
pub struct HeadlessGpu {
    pub adapter_info: wgpu::AdapterInfo,
    pub adapter: wgpu::Adapter,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub uncaptured: UncapturedErrors,
}

impl HeadlessGpu {
    /// Creates an instance, picks an adapter (honouring `WGPU_BACKEND` and
    /// `WGPU_ADAPTER_NAME`) and opens a device. Errors if there is no adapter.
    pub fn new() -> Result<Self, GpuError> {
        Self::open(wgpu::Limits::default())
    }

    /// Like [`new`](Self::new) but with exactly the features and limits `iced_wgpu` 0.14's
    /// compositor requests (no features, default limits with `max_bind_groups: 2` and
    /// `max_non_sampler_bindings: 2048`), to prove vello works on an Iced-created device.
    pub fn iced_like() -> Result<Self, GpuError> {
        Self::open(wgpu::Limits {
            max_bind_groups: 2,
            max_non_sampler_bindings: 2048,
            ..wgpu::Limits::default()
        })
    }

    /// Capability snapshot of this device's adapter.
    pub fn caps(&self) -> GpuCaps {
        GpuCaps::from_adapter(&self.adapter)
    }

    /// Adapter report for the Settings panel.
    pub fn report(&self) -> GpuReport {
        probe(&self.adapter)
    }

    /// Whether this adapter is a software rasteriser: WARP on Windows, llvmpipe on Linux,
    /// SwiftShader and the like. vello crashes inside some of them rather than drawing the
    /// wrong pixels, so the GPU tests skip them instead of taking the process down.
    pub fn is_software_adapter(&self) -> bool {
        let name = self.adapter_info.name.to_ascii_lowercase();
        matches!(self.adapter_info.device_type, wgpu::DeviceType::Cpu)
            || [
                "warp",
                "llvmpipe",
                "swiftshader",
                "basic render driver",
                "software",
            ]
            .iter()
            .any(|needle| name.contains(needle))
    }

    /// An adapter for tests: `None` when there is none, or when it is a software rasteriser.
    pub fn for_tests() -> Option<Self> {
        Self::new().ok().filter(|gpu| !gpu.is_software_adapter())
    }

    /// [`for_tests`](Self::for_tests) with the limits `iced_wgpu` requests.
    pub fn for_tests_iced_like() -> Option<Self> {
        Self::iced_like()
            .ok()
            .filter(|gpu| !gpu.is_software_adapter())
    }

    fn open(required_limits: wgpu::Limits) -> Result<Self, GpuError> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::from_env_or_default());
        let adapter = pollster::block_on(wgpu::util::initialize_adapter_from_env_or_default(
            &instance, None,
        ))
        .map_err(|e| GpuError::NoAdapter(e.to_string()))?;
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("libertum-headless"),
            required_features: wgpu::Features::empty(),
            required_limits,
            ..Default::default()
        }))
        .map_err(|e| GpuError::Device(e.to_string()))?;
        let uncaptured = UncapturedErrors::default();
        install_nonfatal_error_handler(&device, &uncaptured);
        Ok(Self {
            adapter_info: adapter.get_info(),
            adapter,
            device,
            queue,
            uncaptured,
        })
    }
}

/// Encodes page `page_index` into a vello scene in page-point units (origin top-left, white
/// background rectangle included). `None` if the page does not exist.
pub fn build_scene(state: &LayoutState, page_index: usize) -> Option<Scene> {
    let page = state.pages().get(page_index)?;
    let geo = state.page_geometry(page_index);
    let mut scene = Scene::new();
    let paper = match state.page_colour(page_index) {
        Some([r, g, b, _]) => Color::from_rgba8(r, g, b, 255),
        None => Color::WHITE,
    };
    scene.fill(
        Fill::NonZero,
        Affine::IDENTITY,
        paper,
        None,
        &Rect::new(0.0, 0.0, f64::from(geo.width_pt), f64::from(geo.height_pt)),
    );
    let mut coords: Vec<vello::NormalizedCoord> = Vec::new();
    for slice in &page.slices {
        let Some(pl) = state.paragraph_layout(slice.para) else {
            continue;
        };
        let Some(first) = pl.lines.get(slice.line_start) else {
            continue;
        };
        let dy = geo.margins.top + slice.y - first.top;
        let dx = geo.margins.left + slice.x + pl.x_offset;
        encode_lines(
            &mut scene,
            &mut coords,
            &pl,
            slice.line_start..slice.line_end,
            dx,
            dy,
        );
    }
    for decoration in state.page_decorations(page_index) {
        match decoration {
            Decoration::Text { layout, x, y, .. } => {
                encode_lines(
                    &mut scene,
                    &mut coords,
                    &layout,
                    0..layout.lines.len(),
                    x,
                    y,
                );
            }
            Decoration::Leader { x0, x1, baseline } => {
                // Same dots as the CPU renderer (`renderer::draw_leader`).
                let dot = 0.9f64;
                let mut x = f64::from(x1) - dot;
                while x >= f64::from(x0) {
                    let y = f64::from(baseline) - dot;
                    scene.fill(
                        Fill::NonZero,
                        Affine::IDENTITY,
                        Color::BLACK,
                        None,
                        &Rect::new(x, y, x + dot, y + dot),
                    );
                    x -= f64::from(LEADER_PITCH_PT);
                }
            }
            Decoration::Image {
                media,
                x,
                y,
                width,
                height,
            } => {
                let target = Rect::new(
                    f64::from(x),
                    f64::from(y),
                    f64::from(x + width),
                    f64::from(y + height),
                );
                match crate::images::decode(&media) {
                    Some(decoded) => {
                        let image = ImageData {
                            data: Blob::new(Arc::new(decoded.rgba)),
                            format: ImageFormat::Rgba8,
                            alpha_type: ImageAlphaType::Alpha,
                            width: decoded.width,
                            height: decoded.height,
                        };
                        let transform = Affine::translate((f64::from(x), f64::from(y)))
                            * Affine::scale_non_uniform(
                                f64::from(width) / f64::from(decoded.width),
                                f64::from(height) / f64::from(decoded.height),
                            );
                        scene.draw_image(&image, transform);
                    }
                    None => {
                        // Same placeholder as the CPU renderer.
                        let c = crate::renderer::IMAGE_PLACEHOLDER;
                        scene.fill(
                            Fill::NonZero,
                            Affine::IDENTITY,
                            Color::from_rgba8(c[0], c[1], c[2], c[3]),
                            None,
                            &target,
                        );
                    }
                }
            }
            Decoration::Rect {
                x,
                y,
                width,
                height,
                colour,
            } => {
                scene.fill(
                    Fill::NonZero,
                    Affine::IDENTITY,
                    Color::from_rgba8(colour[0], colour[1], colour[2], colour[3]),
                    None,
                    &Rect::new(
                        f64::from(x),
                        f64::from(y),
                        f64::from(x + width),
                        f64::from(y + height),
                    ),
                );
            }
        }
    }
    Some(scene)
}

/// Encodes lines `lines` of `pl` with the layout origin at (`dx`, `dy`) page points.
fn encode_lines(
    scene: &mut Scene,
    coords: &mut Vec<vello::NormalizedCoord>,
    pl: &ParagraphLayout,
    lines: std::ops::Range<usize>,
    dx: f32,
    dy: f32,
) {
    {
        let origin = Affine::translate((f64::from(dx), f64::from(dy)));
        for li in lines {
            let Some(line) = pl.layout.get(li) else {
                continue;
            };
            // Same order as the CPU renderer: highlights, glyphs, then decorations.
            if let Some(lb) = pl.lines.get(li) {
                for item in line.items() {
                    if let PositionedLayoutItem::GlyphRun(run) = item
                        && let Some(hl) = run.style().brush.highlight
                    {
                        fill(
                            scene,
                            dx + run.offset(),
                            dy + lb.top,
                            run.advance(),
                            lb.height,
                            hl,
                        );
                    }
                }
            }
            for item in line.items() {
                let PositionedLayoutItem::GlyphRun(run) = item else {
                    continue;
                };
                let brush = run.style().brush;
                let c = brush.color;
                let color = Color::from_rgba8(c[0], c[1], c[2], c[3]);
                // Superscript and subscript glyphs sit above or below the line's baseline.
                let rise = brush.rise_pt();
                let r = run.run();
                coords.clear();
                coords.extend(r.normalized_coords().iter().map(|c| c.to_bits()));
                // Synthetic oblique: shear x by tan(angle). Synthetic embolden is not supported
                // by vello's glyph API, so such fonts render at regular weight.
                let skew = r
                    .synthesis()
                    .skew()
                    .map(|deg| Affine::skew(f64::from(deg).to_radians().tan(), 0.0));
                scene
                    .draw_glyphs(&outline_only_font(r.font()))
                    .transform(origin * Affine::translate((0.0, -f64::from(rise))))
                    .glyph_transform(skew)
                    .font_size(r.font_size())
                    .normalized_coords(coords)
                    .brush(color)
                    .draw(
                        Fill::NonZero,
                        run.positioned_glyphs().map(|g| Glyph {
                            id: g.id,
                            x: g.x,
                            y: g.y,
                        }),
                    );
                let style = run.style();
                let metrics = r.font_metrics();
                let decorations = [
                    (
                        &style.underline,
                        metrics.underline_offset,
                        metrics.underline_size,
                    ),
                    (
                        &style.strikethrough,
                        metrics.strikethrough_offset,
                        metrics.strikethrough_size,
                    ),
                ];
                for (decoration, offset, size) in decorations {
                    if let Some(d) = decoration {
                        let offset = d.offset.unwrap_or(offset);
                        let size = d.size.unwrap_or(size).max(0.5);
                        let top = dy + run.baseline() - rise - offset - size * 0.5;
                        fill(
                            scene,
                            dx + run.offset(),
                            top,
                            run.advance(),
                            size,
                            d.brush.color,
                        );
                    }
                }
            }
        }
    }
}

/// Fills a rectangle in page points with a straight-alpha colour.
fn fill(scene: &mut Scene, x: f32, y: f32, w: f32, h: f32, c: [u8; 4]) {
    let (x, y) = (f64::from(x), f64::from(y));
    scene.fill(
        Fill::NonZero,
        Affine::IDENTITY,
        Color::from_rgba8(c[0], c[1], c[2], c[3]),
        None,
        &Rect::new(x, y, x + f64::from(w), y + f64::from(h)),
    );
}

/// Pixel size of a page at `scale`, matching the CPU renderer.
pub fn page_pixel_size(state: &LayoutState, page: usize, scale: f32) -> (u32, u32) {
    let geo = state.page_geometry(page);
    (
        (geo.width_pt * scale).ceil().max(1.0) as u32,
        (geo.height_pt * scale).ceil().max(1.0) as u32,
    )
}

/// Pixel dimensions and tightly packed RGBA8 data read back from the GPU.
pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

impl Rgba {
    /// Converts to a tiny-skia pixmap (the page is opaque, so premultiplication is a no-op).
    pub fn into_pixmap(self) -> Option<Pixmap> {
        let mut pm = Pixmap::new(self.width, self.height)?;
        pm.data_mut().copy_from_slice(&self.data);
        Some(pm)
    }
}

const SCENE_CACHE_PAGES: usize = 64;

fn create_target_texture(
    device: &wgpu::Device,
    width: u32,
    height: u32,
    with_srgb_view: bool,
) -> wgpu::Texture {
    // A blit pass samples the page through an sRGB view so it composes correctly on an sRGB
    // surface (see `page_view_format`). OpenGL-class devices lack the `VIEW_FORMATS` capability.
    const SRGB: &[wgpu::TextureFormat] = &[wgpu::TextureFormat::Rgba8UnormSrgb];
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("libertum-page"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: TARGET_FORMAT,
        usage: wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_SRC,
        view_formats: if with_srgb_view { SRGB } else { &[] },
    })
}

/// Options for creating a [`VelloPageRenderer`].
#[derive(Clone, Debug, Default)]
pub struct InitOptions {
    /// Capability snapshot to gate on. `None` uses [`GpuCaps::from_device`] (limits and features
    /// only); pass [`GpuCaps::from_adapter`] whenever the adapter is known, so the backend and
    /// downlevel flags are checked too.
    pub caps: Option<GpuCaps>,
    pub policy: GatePolicy,
    /// Threads for shader compilation. `None`: [`VelloPageRenderer::new`] uses vello's own
    /// heuristic (`available_parallelism - 2`, at least 2); [`init_in_background`] uses
    /// `min(4, cores - 1)`, because on DX12 (FXC) more than 4 threads gained nothing here
    /// (6.2 s at 4 threads, 6.3 s at 8, 6.4 s at 12: one slow shader sets the floor) and the UI
    /// thread should keep a core. On macOS the default is 1 in both cases (vello's advice).
    ///
    /// [`init_in_background`]: VelloPageRenderer::init_in_background
    pub num_init_threads: Option<NonZeroUsize>,
}

/// vello recommends a single shader-compile thread on macOS (`vello-0.7.0/src/lib.rs` docs on
/// `RendererOptions::num_init_threads`, and its `Default` impl), so that is the default there.
const MACOS_INIT_THREADS: Option<NonZeroUsize> = if cfg!(target_os = "macos") {
    NonZeroUsize::new(1)
} else {
    None
};

/// Runs `f` inside Validation, OutOfMemory and Internal error scopes on `device` and returns its
/// value, or the first error any scope caught (or a panic inside `f`). Scopes belong to the
/// *device*, not the thread (wgpu 27 keeps one error sink per device), so errors raised by other
/// threads using the same device during `f` (for example Iced drawing) are also attributed to
/// `f`.
fn with_error_scopes<T>(device: &wgpu::Device, f: impl FnOnce() -> T) -> Result<T, GpuUnavailable> {
    use wgpu::ErrorFilter::{Internal, OutOfMemory, Validation};
    for filter in [Validation, OutOfMemory, Internal] {
        device.push_error_scope(filter);
    }
    let value = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    // Pop in reverse order; on native the future is ready immediately.
    let mut first_error: Option<wgpu::Error> = None;
    for _ in 0..3 {
        let e = pollster::block_on(device.pop_error_scope());
        if first_error.is_none() {
            first_error = e;
        }
    }
    match (value, first_error) {
        (Err(panic), _) => Err(GpuUnavailable::new(format!(
            "GPU code panicked: {}",
            panic_message(panic.as_ref())
        ))),
        (Ok(_), Some(e)) => Err(GpuUnavailable::new(format!("wgpu error: {e}"))),
        (Ok(v), None) => Ok(v),
    }
}

fn panic_message(p: &(dyn std::any::Any + Send)) -> String {
    p.downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| p.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown".to_owned())
}

/// vello-based page renderer. Owns a (cloned, ref-counted) wgpu device and queue, so it can use
/// the device Iced already created.
pub struct VelloPageRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    renderer: Renderer,
    scenes: HashMap<(usize, u64), Arc<Scene>>,
    order: VecDeque<(usize, u64)>,
    root: Scene,
    /// Whether page textures can list an sRGB view format (downlevel `VIEW_FORMATS`).
    view_formats_supported: bool,
    pub scene_hits: u64,
    pub scene_misses: u64,
}

impl VelloPageRenderer {
    /// Creates the vello renderer on `device` with default [`InitOptions`]. Never panics: an
    /// unsupported GPU, a wgpu validation, out-of-memory or internal error, or a panic inside
    /// init all come back as [`GpuUnavailable`]. Blocks while shaders compile (see
    /// [`init_in_background`](Self::init_in_background); about 6 s on DX12).
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> Result<Self, GpuUnavailable> {
        Self::new_with(device, queue, &InitOptions::default())
    }

    /// As [`new`](Self::new), with options. Steps: capability gate, then (inside error scopes)
    /// vello `Renderer::new` with area anti-aliasing only, then a smoke render of a 64 x 64
    /// scene that is read back and checked, so a driver that accepts shaders but draws garbage
    /// is also reported unavailable.
    pub fn new_with(
        device: wgpu::Device,
        queue: wgpu::Queue,
        options: &InitOptions,
    ) -> Result<Self, GpuUnavailable> {
        let caps = options
            .caps
            .clone()
            .unwrap_or_else(|| GpuCaps::from_device(&device));
        check_caps(&caps, options.policy)?;

        // Probe whether a texture may list an sRGB view format. This is a downlevel capability
        // that a `wgpu::Device` does not expose, so create one and see.
        let view_formats_supported = with_error_scopes(&device, || {
            create_target_texture(&device, 1, 1, true).destroy();
        })
        .is_ok();

        let renderer = with_error_scopes(&device, || {
            Renderer::new(
                &device,
                RendererOptions {
                    use_cpu: false,
                    antialiasing_support: AaSupport::area_only(),
                    num_init_threads: options.num_init_threads.or(MACOS_INIT_THREADS),
                    pipeline_cache: None,
                },
            )
        })?
        .map_err(|e| GpuUnavailable::new(format!("vello renderer: {e}")))?;

        let mut this = Self {
            device,
            queue,
            renderer,
            scenes: HashMap::new(),
            order: VecDeque::new(),
            root: Scene::new(),
            view_formats_supported,
            scene_hits: 0,
            scene_misses: 0,
        };
        this.smoke_test()?;
        Ok(this)
    }

    /// Creates the renderer on a background thread so the UI never blocks on shader
    /// compilation. The receiver yields exactly one message. Until it arrives, show CPU pages.
    ///
    /// `device` and `queue` are ref-counted handles (clone Iced's). If the receiver is dropped
    /// the result is discarded. Caveat for the in-app case: the work runs inside device-wide
    /// error scopes, so a wgpu error raised by Iced on the same device during that window (it
    /// can last about 6 s on DX12) is caught here and reported as an init failure, which
    /// falls back to CPU rather than aborting.
    pub fn init_in_background(
        device: wgpu::Device,
        queue: wgpu::Queue,
        mut options: InitOptions,
    ) -> std::sync::mpsc::Receiver<Result<Self, GpuUnavailable>> {
        if options.num_init_threads.is_none() {
            options.num_init_threads = MACOS_INIT_THREADS;
        }
        if options.num_init_threads.is_none() {
            let cores = std::thread::available_parallelism().map_or(2, NonZeroUsize::get);
            options.num_init_threads = NonZeroUsize::new(cores.saturating_sub(1).clamp(1, 4));
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let worker_tx = tx.clone();
        let spawned = std::thread::Builder::new()
            .name("libertum-vello-init".into())
            .spawn(move || {
                let _ = worker_tx.send(Self::new_with(device, queue, &options));
            });
        if let Err(e) = spawned {
            let _ = tx.send(Err(GpuUnavailable::new(format!(
                "cannot start the GPU init thread: {e}"
            ))));
        }
        rx
    }

    /// Renders 64 x 64 (left half red, right half the white base colour), reads it back and
    /// checks two pixels.
    fn smoke_test(&mut self) -> Result<(), GpuUnavailable> {
        let device = self.device.clone();
        with_error_scopes(&device, || -> Result<(), GpuUnavailable> {
            let mut scene = Scene::new();
            scene.fill(
                Fill::NonZero,
                Affine::IDENTITY,
                Color::from_rgba8(255, 0, 0, 255),
                None,
                &Rect::new(0.0, 0.0, 32.0, 64.0),
            );
            let tex = create_target_texture(&self.device, 64, 64, self.view_formats_supported);
            let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
            self.renderer
                .render_to_texture(
                    &self.device,
                    &self.queue,
                    &scene,
                    &view,
                    &RenderParams {
                        base_color: Color::WHITE,
                        width: 64,
                        height: 64,
                        antialiasing_method: AaConfig::Area,
                    },
                )
                .map_err(|e| GpuUnavailable::new(format!("smoke render: {e}")))?;
            let img = self
                .read_back(&tex)
                .map_err(|e| GpuUnavailable::new(format!("smoke readback: {e}")))?;
            let px = |x: usize, y: usize| {
                let o = (y * 64 + x) * 4;
                [
                    img.data[o],
                    img.data[o + 1],
                    img.data[o + 2],
                    img.data[o + 3],
                ]
            };
            if px(8, 32) != [255, 0, 0, 255] || px(56, 32) != [255, 255, 255, 255] {
                return Err(GpuUnavailable::new(format!(
                    "smoke render produced wrong pixels: {:?} and {:?}",
                    px(8, 32),
                    px(56, 32)
                )));
            }
            Ok(())
        })?
    }

    /// True when page textures carry an sRGB view format, so a blit can sample them through
    /// [`page_view_format`]. When false (OpenGL-class devices), sample the plain `Rgba8Unorm`
    /// view and decode sRGB in the blit shader if the target is sRGB.
    pub fn supports_srgb_view(&self) -> bool {
        self.view_formats_supported
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    /// Cached scene for the page, rebuilt when the page content hash changed.
    pub fn scene_for_page(&mut self, state: &LayoutState, page_index: usize) -> Option<Arc<Scene>> {
        let key = (page_index, state.page_content_hash(page_index)?);
        if let Some(s) = self.scenes.get(&key) {
            self.scene_hits += 1;
            return Some(s.clone());
        }
        self.scene_misses += 1;
        let scene = Arc::new(build_scene(state, page_index)?);
        // Drop stale versions of the same page, then enforce the cap (oldest first).
        let scenes = &mut self.scenes;
        self.order.retain(|k| {
            let keep = k.0 != page_index;
            if !keep {
                scenes.remove(k);
            }
            keep
        });
        while self.order.len() >= SCENE_CACHE_PAGES {
            if let Some(old) = self.order.pop_front() {
                self.scenes.remove(&old);
            }
        }
        self.scenes.insert(key, scene.clone());
        self.order.push_back(key);
        Some(scene)
    }

    /// Creates a render target: `Rgba8Unorm`, usable by vello (storage), sampling and readback.
    /// It lists `Rgba8UnormSrgb` as a view format when the device allows that (see
    /// [`supports_srgb_view`](Self::supports_srgb_view)); create the sampled view with
    /// [`sampled_view`](Self::sampled_view).
    pub fn create_target(&self, width: u32, height: u32) -> wgpu::Texture {
        create_target_texture(&self.device, width, height, self.view_formats_supported)
    }

    /// The view a blit should sample for a render target of `target_format` (Iced's surface
    /// format): the sRGB view when the target is sRGB, else the plain view (see
    /// [`page_view_format`]). `None` when the target is sRGB but the device cannot make an sRGB
    /// view; then sample the plain `Rgba8Unorm` view and decode sRGB in the shader.
    pub fn sampled_view(
        &self,
        texture: &wgpu::Texture,
        target_format: wgpu::TextureFormat,
    ) -> Option<wgpu::TextureView> {
        let fmt = page_view_format(target_format);
        if fmt != TARGET_FORMAT && !self.view_formats_supported {
            return None;
        }
        Some(texture.create_view(&wgpu::TextureViewDescriptor {
            format: Some(fmt),
            ..Default::default()
        }))
    }

    /// Renders the page at `scale` into `target`, which must be `page_pixel_size` large and
    /// created like [`create_target`](Self::create_target). Submits work but does not wait.
    /// wgpu errors during the call (validation, out of memory, internal) are returned as
    /// [`GpuError::Renderer`] instead of aborting.
    pub fn render_into(
        &mut self,
        state: &LayoutState,
        page_index: usize,
        scale: f32,
        target: &wgpu::TextureView,
    ) -> Result<(), GpuError> {
        let scene = self
            .scene_for_page(state, page_index)
            .ok_or_else(|| GpuError::Renderer(format!("page {page_index} does not exist")))?;
        let (width, height) = page_pixel_size(state, page_index, scale);
        self.root.reset();
        self.root
            .append(&scene, Some(Affine::scale(f64::from(scale))));
        let device = self.device.clone();
        with_error_scopes(&device, || {
            self.renderer.render_to_texture(
                &self.device,
                &self.queue,
                &self.root,
                target,
                &RenderParams {
                    base_color: Color::WHITE,
                    width,
                    height,
                    antialiasing_method: AaConfig::Area,
                },
            )
        })
        .map_err(|e| GpuError::Renderer(e.reason))?
        .map_err(|e| GpuError::Renderer(e.to_string()))
    }

    /// Renders into a fresh texture and returns it.
    pub fn render_texture(
        &mut self,
        state: &LayoutState,
        page_index: usize,
        scale: f32,
    ) -> Result<wgpu::Texture, GpuError> {
        let (w, h) = page_pixel_size(state, page_index, scale);
        let tex = self.create_target(w, h);
        let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
        self.render_into(state, page_index, scale, &view)?;
        Ok(tex)
    }

    /// Blocks until all submitted GPU work has finished (for timing).
    pub fn wait_idle(&self) {
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
    }

    /// Copies `texture` (must have `COPY_SRC`) back to CPU memory as tight RGBA8.
    pub fn read_back(&self, texture: &wgpu::Texture) -> Result<Rgba, GpuError> {
        let size = texture.size();
        let (width, height) = (size.width, size.height);
        let unpadded = width * 4;
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let padded = unpadded.div_ceil(align) * align;
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("libertum-readback"),
            size: u64::from(padded) * u64::from(height),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("libertum-readback"),
            });
        enc.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded),
                    rows_per_image: Some(height),
                },
            },
            size,
        );
        self.queue.submit([enc.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer.slice(..).map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .map_err(|e| GpuError::Readback(e.to_string()))?;
        rx.recv()
            .map_err(|e| GpuError::Readback(e.to_string()))?
            .map_err(|e| GpuError::Readback(e.to_string()))?;
        let mapped = buffer.slice(..).get_mapped_range();
        let mut data = Vec::with_capacity((unpadded * height) as usize);
        for row in mapped.chunks(padded as usize).take(height as usize) {
            data.extend_from_slice(&row[..unpadded as usize]);
        }
        drop(mapped);
        buffer.unmap();
        Ok(Rgba {
            width,
            height,
            data,
        })
    }

    /// Render + wait + read back (tests and comparison).
    pub fn render_to_rgba(
        &mut self,
        state: &LayoutState,
        page_index: usize,
        scale: f32,
    ) -> Result<Rgba, GpuError> {
        let tex = self.render_texture(state, page_index, scale)?;
        self.read_back(&tex)
    }
}

impl PageBackend for VelloPageRenderer {
    fn name(&self) -> &'static str {
        "vello"
    }

    fn render(&mut self, state: &LayoutState, page_index: usize, scale: f32) -> Option<PageOutput> {
        self.render_texture(state, page_index, scale)
            .ok()
            .map(|t| PageOutput::Texture(Arc::new(t)))
    }
}

/// vello 0.7 draws a font's *embedded monochrome bitmap strikes* (EBLC/EBDT, e.g. Cambria at
/// small sizes) whenever one exists for the requested size, and renders them as white images
/// ignoring the brush colour, so most text disappears on a white page. This returns a copy of
/// the font whose `EBLC` table is renamed (`EBLB`, which keeps the directory sorted), so vello
/// finds no strikes and uses the outlines, as the CPU renderer does. Colour bitmap fonts
/// (CBLC/sbix) are left alone. Returns `None` when the font has no `EBLC` table.
fn without_embedded_bitmaps(font: &vello::peniko::FontData) -> Option<vello::peniko::FontData> {
    let d = font.data.data();
    let be32 = |o: usize| -> Option<usize> {
        let b: [u8; 4] = d.get(o..o + 4)?.try_into().ok()?;
        Some(u32::from_be_bytes(b) as usize)
    };
    let dir = if d.get(0..4)? == b"ttcf" {
        be32(12 + 4 * font.index as usize)?
    } else {
        0
    };
    let n = u16::from_be_bytes(d.get(dir + 4..dir + 6)?.try_into().ok()?) as usize;
    for i in 0..n {
        let rec = dir + 12 + i * 16;
        if d.get(rec..rec + 4)? == b"EBLC" {
            let mut copy = d.to_vec();
            copy[rec + 3] = b'B';
            let blob = vello::peniko::Blob::new(Arc::new(copy));
            return Some(vello::peniko::FontData::new(blob, font.index));
        }
    }
    None
}

/// Cache of fixed-up fonts keyed by the original blob id.
fn outline_only_font(font: &vello::peniko::FontData) -> vello::peniko::FontData {
    use std::sync::{Mutex, OnceLock};
    static CACHE: OnceLock<Mutex<HashMap<(u64, u32), vello::peniko::FontData>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let key = (font.data.id(), font.index);
    let Ok(mut map) = cache.lock() else {
        return font.clone();
    };
    map.entry(key)
        .or_insert_with(|| without_embedded_bitmaps(font).unwrap_or_else(|| font.clone()))
        .clone()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_scopes_turn_validation_errors_into_values_not_panics() {
        let Ok(gpu) = HeadlessGpu::new() else {
            eprintln!("no GPU adapter; skipping");
            return;
        };
        // A zero-sized texture is a validation error; without a scope wgpu 27 would panic.
        let r = with_error_scopes(&gpu.device, || {
            gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("invalid"),
                size: wgpu::Extent3d {
                    width: 0,
                    height: 0,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: TARGET_FORMAT,
                usage: wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
        });
        let err = r.expect_err("a validation error must be returned");
        assert!(err.reason.contains("wgpu error"), "{}", err.reason);
        // The scopes are balanced: a valid call afterwards succeeds.
        assert!(with_error_scopes(&gpu.device, || 1 + 1).is_ok());
    }

    #[test]
    fn panics_inside_scopes_become_errors() {
        let Some(gpu) = HeadlessGpu::for_tests() else {
            return;
        };
        let r = with_error_scopes(&gpu.device, || -> u32 { panic!("boom") });
        let err = r.expect_err("panic must be caught");
        assert!(err.reason.contains("boom"), "{}", err.reason);
    }

    #[test]
    fn gate_rejects_before_touching_the_device() {
        let Some(gpu) = HeadlessGpu::for_tests() else {
            return;
        };
        let mut caps = gpu.caps();
        caps.backend = Some(wgpu::Backend::Gl);
        let r = VelloPageRenderer::new_with(
            gpu.device.clone(),
            gpu.queue.clone(),
            &InitOptions {
                caps: Some(caps),
                ..InitOptions::default()
            },
        );
        let err = r.err().expect("Gl must be rejected");
        assert!(err.reason.contains("OpenGL"), "{}", err.reason);
    }

    #[test]
    fn background_init_sends_exactly_one_result() {
        let Some(gpu) = HeadlessGpu::for_tests() else {
            return;
        };
        let rx = VelloPageRenderer::init_in_background(
            gpu.device.clone(),
            gpu.queue.clone(),
            InitOptions {
                caps: Some(gpu.caps()),
                ..InitOptions::default()
            },
        );
        let first = rx
            .recv_timeout(std::time::Duration::from_secs(120))
            .expect("init result");
        // Supported or not, it is a value, and the sender is gone afterwards.
        match first {
            Ok(r) => assert!(r.device().limits().max_bind_groups >= 1),
            Err(e) => assert!(!e.reason.is_empty()),
        }
        assert!(rx.recv().is_err());
    }

    #[test]
    fn backend_is_recovered_from_a_bare_device() {
        let Some(gpu) = HeadlessGpu::for_tests() else {
            return;
        };
        assert_eq!(
            GpuCaps::from_device(&gpu.device).backend,
            Some(gpu.adapter_info.backend)
        );
    }

    #[test]
    fn probe_default_does_not_panic_and_is_consistent() {
        if let Ok(report) = probe_default() {
            assert_eq!(report.supported, report.reason.is_none());
            assert!(!report.adapter_name.is_empty());
        }
    }
}
