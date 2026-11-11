//! Measures vello `Renderer::new` (shader compilation) under different options. Run with
//!
//!     WGPU_BACKEND=dx12 cargo run --release -p libertum-render --features gpu --example gpu_init_bench
//!
//! Environment: `WGPU_BACKEND`, `WGPU_DX12_COMPILER` (fxc | dxc | staticdxc), `WGPU_DEBUG`,
//! `BENCH_THREADS` (comma list, default `0,1,2,4`; 0 = vello's own heuristic),
//! `BENCH_AA` (`area` default, or `all`), `BENCH_CPU=1` (vello `use_cpu`).
#![allow(clippy::print_stdout)]

#[cfg(not(feature = "gpu"))]
fn main() {
    println!("gpu_init_bench needs `--features gpu`");
}

#[cfg(feature = "gpu")]
fn main() {
    use std::num::NonZeroUsize;
    use std::time::Instant;

    use libertum_render::gpu::HeadlessGpu;
    use vello::{AaSupport, Renderer, RendererOptions};

    let var = |k: &str| std::env::var(k).unwrap_or_default();
    let threads: Vec<usize> = std::env::var("BENCH_THREADS")
        .unwrap_or_else(|_| "0,1,2,4".into())
        .split(',')
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    let aa = if var("BENCH_AA") == "all" {
        AaSupport::all()
    } else {
        AaSupport::area_only()
    };
    let use_cpu = var("BENCH_CPU") == "1";
    println!(
        "WGPU_BACKEND={} WGPU_DX12_COMPILER={} WGPU_DEBUG={} aa={} use_cpu={use_cpu} cores={}",
        var("WGPU_BACKEND"),
        var("WGPU_DX12_COMPILER"),
        var("WGPU_DEBUG"),
        if var("BENCH_AA") == "all" {
            "all"
        } else {
            "area"
        },
        std::thread::available_parallelism().map_or(0, |n| n.get()),
    );
    for &n in &threads {
        // A fresh device each time, so no driver-side shader cache is shared through our handles.
        let gpu = match HeadlessGpu::new() {
            Ok(g) => g,
            Err(e) => {
                println!("no GPU: {e}");
                return;
            }
        };
        let t = Instant::now();
        let r = Renderer::new(
            &gpu.device,
            RendererOptions {
                use_cpu,
                antialiasing_support: aa,
                num_init_threads: NonZeroUsize::new(n),
                pipeline_cache: None,
            },
        );
        let ms = t.elapsed().as_secs_f64() * 1000.0;
        println!(
            "{:?} init_threads={n}: Renderer::new {ms:.0} ms ({})",
            gpu.adapter_info.backend,
            if r.is_ok() { "ok" } else { "ERR" }
        );
        println!("   uncaptured: {:?}", gpu.uncaptured.take());
    }
}
