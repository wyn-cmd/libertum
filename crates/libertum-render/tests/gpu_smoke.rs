//! CPU vs GPU parity (PLAN §4.5). Skips when there is no GPU adapter or no system fonts.
#![cfg(feature = "gpu")]

use libertum_layout::spike::generate_document;
use libertum_layout::{LayoutOptions, LayoutState, PageGeometry};
use libertum_render::gpu::{HeadlessGpu, VelloPageRenderer};
use libertum_render::{PageBackend, PageOutput, PageRenderer};
use vello::wgpu::Backend;

fn ink(data: &[u8]) -> usize {
    data.chunks_exact(4).filter(|p| p[0] < 200).count()
}

#[test]
fn gpu_matches_cpu_within_tolerance() {
    let mut state = LayoutState::new(
        PageGeometry::a4(),
        generate_document(3, 5),
        LayoutOptions::default(),
    );
    state.paginate_all();
    if state.font_report().family_count == 0 {
        return;
    }
    let Some(gpu) = HeadlessGpu::for_tests() else {
        eprintln!("no hardware GPU adapter; skipping");
        return;
    };
    let Ok(mut vello) = VelloPageRenderer::new(gpu.device.clone(), gpu.queue.clone()) else {
        eprintln!("vello init failed; skipping");
        return;
    };
    let mut cpu = PageRenderer::default();
    for scale in [1.0f32, 2.0] {
        let c = cpu.render_page(&state, 0, scale).expect("cpu");
        let g = vello.render_to_rgba(&state, 0, scale).expect("gpu");
        assert_eq!((c.width(), c.height()), (g.width, g.height));
        let mad = c
            .data()
            .chunks_exact(4)
            .zip(g.data.chunks_exact(4))
            .map(|(a, b)| (0..3).map(|i| u64::from(a[i].abs_diff(b[i]))).sum::<u64>())
            .sum::<u64>() as f64
            / (c.data().len() as f64 / 4.0 * 3.0);
        assert!(mad < 10.0, "mean abs diff {mad} at {scale}x");
        let (ci, gi) = (ink(c.data()) as f64, ink(&g.data) as f64);
        assert!(
            (gi / ci - 1.0).abs() < 0.15,
            "ink coverage cpu {ci} gpu {gi} at {scale}x"
        );
        // Opaque white page background in the margin corner.
        assert_eq!(&g.data[0..4], &[255, 255, 255, 255]);
    }
}

#[test]
fn backend_trait_works_for_both() {
    let mut state = LayoutState::new(
        PageGeometry::a4(),
        generate_document(2, 1),
        LayoutOptions::default(),
    );
    state.paginate_all();
    let mut cpu = PageRenderer::default();
    assert!(matches!(
        cpu.render(&state, 0, 1.0),
        Some(PageOutput::Bitmap(_))
    ));
    assert!(cpu.render(&state, 9999, 1.0).is_none());
    let Ok(gpu) = HeadlessGpu::new() else {
        return;
    };
    let Ok(mut vello) = VelloPageRenderer::new(gpu.device.clone(), gpu.queue.clone()) else {
        return;
    };
    assert!(matches!(
        vello.render(&state, 0, 1.0),
        Some(PageOutput::Texture(_))
    ));
    assert!(vello.render(&state, 9999, 1.0).is_none());
}

#[test]
fn vello_runs_on_iced_style_device() {
    let Some(gpu) = HeadlessGpu::for_tests_iced_like() else {
        return;
    };
    let mut state = LayoutState::new(
        PageGeometry::a4(),
        generate_document(2, 1),
        LayoutOptions::default(),
    );
    state.paginate_all();
    let mut vello = match VelloPageRenderer::new(gpu.device.clone(), gpu.queue.clone()) {
        Ok(v) => v,
        // The gate may refuse (for example OpenGL); that must be a value, never a panic.
        Err(e) => {
            assert!(
                matches!(gpu.adapter_info.backend, Backend::Gl),
                "vello must initialise on an iced-style device: {e}"
            );
            return;
        }
    };
    let img = vello.render_to_rgba(&state, 0, 1.0).expect("render");
    assert_eq!(&img.data[0..4], &[255, 255, 255, 255]);
}
