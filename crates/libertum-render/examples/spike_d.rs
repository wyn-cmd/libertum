//! Spike D: CPU (tiny-skia + swash) vs GPU (vello) page rendering. Run with:
//!
//!     cargo run --release -p libertum-render --features gpu --example spike_d
//!
//! Writes PNGs to `<workspace>/target/spike_d/`. Skips gracefully when no GPU adapter exists.
#![allow(clippy::print_stdout)]

#[cfg(not(feature = "gpu"))]
fn main() {
    println!("spike_d needs `--features gpu`");
}

#[cfg(feature = "gpu")]
fn main() {
    gpu::run();
}

#[cfg(feature = "gpu")]
mod gpu {
    use std::path::PathBuf;
    use std::time::Instant;

    use libertum_layout::spike::generate_document;
    use libertum_layout::{LayoutOptions, LayoutState, PageGeometry};
    use libertum_render::gpu::{
        GatePolicy, HeadlessGpu, InitOptions, VelloPageRenderer, build_scene,
    };
    use libertum_render::{PageRenderer, Pixmap};

    fn ms(t: Instant) -> f64 {
        t.elapsed().as_secs_f64() * 1000.0
    }

    /// (mean absolute difference per RGB channel, % of pixels where any channel differs by > 32)
    fn diff(a: &Pixmap, b: &Pixmap) -> (f64, f64) {
        assert_eq!((a.width(), a.height()), (b.width(), b.height()));
        let (da, db) = (a.data(), b.data());
        let mut sum = 0u64;
        let mut big = 0u64;
        for (pa, pb) in da.chunks_exact(4).zip(db.chunks_exact(4)) {
            let mut mx = 0u8;
            for c in 0..3 {
                let d = pa[c].abs_diff(pb[c]);
                sum += u64::from(d);
                mx = mx.max(d);
            }
            if mx > 32 {
                big += 1;
            }
        }
        let n = (da.len() / 4) as f64;
        (sum as f64 / (n * 3.0), big as f64 * 100.0 / n)
    }

    /// Ink-containing pixels (not near white), to compare coverage between the renderers.
    fn ink(a: &Pixmap) -> f64 {
        a.data().chunks_exact(4).filter(|p| p[0] < 200).count() as f64
    }

    pub fn run() {
        let out_dir: PathBuf = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/spike_d")
            .components()
            .collect();
        if let Err(e) = std::fs::create_dir_all(&out_dir) {
            println!("warning: cannot create {}: {e}", out_dir.display());
        }

        let t = Instant::now();
        let gpu = match HeadlessGpu::new() {
            Ok(g) => g,
            Err(e) => {
                println!("SKIPPED: {e}");
                return;
            }
        };
        let info = &gpu.adapter_info;
        println!(
            "adapter: {} | backend {:?} | type {:?} | driver {} {} | vendor {:#x} device {:#x} (init {:.0} ms)",
            info.name,
            info.backend,
            info.device_type,
            info.driver,
            info.driver_info,
            info.vendor,
            info.device,
            ms(t)
        );

        let report = gpu.report();
        println!("probe: {}", report.summary());
        for note in &report.notes {
            println!("note: {note}");
        }

        // Same path the app uses: init on a background thread, CPU pages meanwhile.
        let t = Instant::now();
        let rx = VelloPageRenderer::init_in_background(
            gpu.device.clone(),
            gpu.queue.clone(),
            InitOptions {
                caps: Some(gpu.caps()),
                // Diagnostic only: LIBERTUM_GPU_ALLOW_GL=1 lifts the OpenGL rejection.
                policy: GatePolicy {
                    allow_experimental_backends: std::env::var("LIBERTUM_GPU_ALLOW_GL").is_ok(),
                    ..GatePolicy::default()
                },
                ..InitOptions::default()
            },
        );
        let mut vello = match rx.recv() {
            Ok(Ok(v)) => v,
            Ok(Err(e)) => {
                println!("GPU UNAVAILABLE (graceful): {e}");
                println!("uncaptured wgpu errors: {:?}", gpu.uncaptured.take());
                return;
            }
            Err(e) => {
                println!("GPU init thread died: {e}");
                return;
            }
        };
        println!(
            "vello init (gate + Renderer::new + smoke render): {:.0} ms",
            ms(t)
        );

        let mut state = LayoutState::new(
            PageGeometry::a4(),
            generate_document(500, 42),
            LayoutOptions::default(),
        );
        let t = Instant::now();
        state.paginate_all();
        println!("layout: {} pages in {:.0} ms", state.pages().len(), ms(t));

        let mut cpu = PageRenderer::default();

        // ---- visual comparison + diff metric ------------------------------------------------
        for &page in &[0usize, 249] {
            for &scale in &[1.0f32, 2.0] {
                let cpu_pm = cpu.render_page(&state, page, scale).expect("cpu page");
                let gpu_px = vello
                    .render_to_rgba(&state, page, scale)
                    .expect("gpu render")
                    .into_pixmap()
                    .expect("pixmap");
                let (mad, pct) = diff(&cpu_pm, &gpu_px);
                println!(
                    "page {:>3} @ {scale}x ({}x{}): mean abs diff {mad:.3}/255, pixels differing >32: {pct:.3}%, ink px cpu {} gpu {}",
                    page + 1,
                    cpu_pm.width(),
                    cpu_pm.height(),
                    ink(&cpu_pm),
                    ink(&gpu_px)
                );
                let tag = format!("page{:03}_{}x", page + 1, scale as u32);
                let _ = cpu_pm.save_png(out_dir.join(format!("{tag}_cpu.png")));
                let _ = gpu_px.save_png(out_dir.join(format!("{tag}_gpu.png")));
            }
        }

        // ---- timings over 20 pages (100..120) -------------------------------------------------
        for &scale in &[1.0f32, 2.0] {
            let pages: Vec<usize> = (100..120).collect();
            // warm-up: first GPU render allocates buffers
            let _ = vello.render_to_rgba(&state, 0, scale);
            let (mut build, mut render, mut read, mut cpu_t) = (0.0, 0.0, 0.0, 0.0);
            for &p in &pages {
                let t = Instant::now();
                let _scene = build_scene(&state, p).expect("scene");
                build += ms(t);

                // scene cached now? measure GPU render alone (cache hit path needs a warm scene)
                let _ = vello.scene_for_page(&state, p);
                let t = Instant::now();
                let tex = vello.render_texture(&state, p, scale).expect("render");
                vello.wait_idle();
                render += ms(t);

                let t = Instant::now();
                let rgba = vello.read_back(&tex).expect("readback");
                read += ms(t);
                std::hint::black_box(rgba.data.len());

                let t = Instant::now();
                let pm = cpu.render_page(&state, p, scale).expect("cpu");
                cpu_t += ms(t);
                std::hint::black_box(pm.width());
            }
            let n = pages.len() as f64;
            println!(
                "{scale}x, avg of {} pages: scene build {:.2} ms | GPU render (scene append + submit + wait) {:.2} ms | readback {:.2} ms | CPU render (warm glyphs) {:.2} ms",
                pages.len(),
                build / n,
                render / n,
                read / n,
                cpu_t / n
            );
        }
        println!(
            "scene cache: {} hits, {} misses",
            vello.scene_hits, vello.scene_misses
        );
        println!("PNGs in {}", out_dir.display());
    }
}
