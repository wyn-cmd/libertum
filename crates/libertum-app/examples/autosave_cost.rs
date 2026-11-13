//! How long the pieces of an autosave take on a generated document (default 500 pages):
//! cloning the model (this runs on the UI thread), then serialising it for the recovery
//! snapshot (this runs on a worker). Run with
//! `cargo run --release -p libertum-app --example autosave_cost -- 500`.

use std::time::Instant;

fn main() {
    let pages: usize = std::env::args()
        .nth(1)
        .and_then(|arg| arg.parse().ok())
        .unwrap_or(500);
    let document = libertum_layout::spike::generate_model_document(pages, 11);
    let mut clones = Vec::new();
    for _ in 0..20 {
        let started = Instant::now();
        let copy = document.clone();
        clones.push(started.elapsed().as_secs_f64() * 1000.0);
        std::hint::black_box(copy);
    }
    clones.sort_by(f64::total_cmp);
    println!(
        "clone, {pages} pages: median {:.2} ms, worst {:.2} ms",
        clones[clones.len() / 2],
        clones[clones.len() - 1]
    );
    let dir = std::env::temp_dir().join("libertum-autosave-cost");
    let mut writes = Vec::new();
    for _ in 0..5 {
        let started = Instant::now();
        let result = libertum_app_recovery_write(&dir, &document);
        writes.push(started.elapsed().as_secs_f64() * 1000.0);
        if let Err(error) = result {
            println!("write failed: {error}");
            return;
        }
    }
    writes.sort_by(f64::total_cmp);
    println!("snapshot write: median {:.1} ms", writes[writes.len() / 2]);
}

/// The same work as the app's recovery snapshot: the native serialisation to a file.
fn libertum_app_recovery_write(
    dir: &std::path::Path,
    document: &libertum_model::Document,
) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|error| error.to_string())?;
    let bytes = libertum_native::Native::new(document.clone())
        .to_bytes()
        .map_err(|error| error.to_string())?;
    std::fs::write(dir.join("snapshot.lbtd"), bytes).map_err(|error| error.to_string())
}
