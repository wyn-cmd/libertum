//! Measures RTF opening and saving on a 500-page-sized generated document.
//!
//! Run with `cargo run --release -p libertum-rtf --example rtf_io_bench`.

use std::time::Instant;

use libertum_model::{
    BlockId, CharFormat, Document, Margins, PageSetup, ParaFormat, ParaProps, Paragraph, Run,
    Section, StyleId, StyleSheet,
};
use libertum_rtf::Rtf;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let paragraphs = std::env::args()
        .nth(1)
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(8_000);

    let mut styles = StyleSheet::with_defaults("Georgia", 12.0);
    styles.para_defaults.space_after_pt = 6.0;
    styles.para_defaults.line_spacing = libertum_model::LineSpacing::Multiple(1.15);

    let mut body = Vec::with_capacity(paragraphs);
    for index in 0..paragraphs {
        let text = format!(
            "Paragraph {index} of the generated RTF benchmark. It contains enough words to wrap over two or three lines in the page, so the stream has a realistic amount of text."
        );
        let bold_start = text.find("generated").unwrap_or(0);
        let bold_end = bold_start + "generated".len();
        let paragraph = if index % 40 == 0 {
            Paragraph::with_runs(
                BlockId(0),
                format!("Chapter {}", index / 40 + 1),
                Vec::new(),
            )
            .with_props(ParaProps {
                style: StyleId::HEADING_1,
                outline_level: Some(1),
                ..ParaProps::default()
            })
        } else if index % 7 == 0 {
            Paragraph::with_runs(
                BlockId(0),
                text,
                vec![Run {
                    range: bold_start..bold_end,
                    format: CharFormat {
                        weight: Some(700),
                        italic: Some(true),
                        ..CharFormat::default()
                    },
                }],
            )
            .with_props(ParaProps {
                format: ParaFormat {
                    space_after_pt: Some(6.0),
                    line_spacing: Some(libertum_model::LineSpacing::Multiple(1.15)),
                    ..ParaFormat::default()
                },
                ..ParaProps::default()
            })
        } else {
            Paragraph::new(BlockId(0), text)
        };
        body.push(libertum_model::Block::Paragraph(paragraph));
    }

    let mut document = Document::from_blocks(body);
    document.styles = styles;
    document.sections = vec![Section {
        page: PageSetup {
            paper: libertum_model::PaperSize::A4,
            orientation: libertum_model::Orientation::Portrait,
            margins: Margins::uniform(72.0),
        },
        ..Section::default()
    }];

    let started = Instant::now();
    let rtf = Rtf::new(document);
    let bytes = rtf.to_bytes()?;
    let save_1 = started.elapsed();

    let started = Instant::now();
    let parsed = Rtf::parse(&bytes)?;
    let open = started.elapsed();

    let started = Instant::now();
    let bytes_2 = parsed.to_bytes()?;
    let save_2 = started.elapsed();

    println!(
        "paragraphs={} bytes={} save={:?} open={:?} save_again={:?} second_bytes={}",
        parsed.document().paragraph_count(),
        bytes.len(),
        save_1,
        open,
        save_2,
        bytes_2.len()
    );
    Ok(())
}
