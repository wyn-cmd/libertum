//! Writes a 20-page native document with a diagonal "DRAFT" watermark, for screenshots:
//! `cargo run -p libertum-app --example watermark_sample`, then open
//! `target/watermark-sample.lbtd` (for example with `LIBERTUM_OPEN`).

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut document = libertum_layout::spike::generate_model_document(20, 3);
    for section in &mut document.sections {
        section.watermark = libertum_model::Watermark::new("DRAFT", true);
    }
    std::fs::create_dir_all("target")?;
    let path = "target/watermark-sample.lbtd";
    let _ = std::fs::remove_file(path);
    libertum_native::Native::new(document).save(path)?;
    println!("wrote {path}");
    Ok(())
}
