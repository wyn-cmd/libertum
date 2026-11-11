//! Image bytes for the importers and the app: PNG and JPEG pass straight into the model,
//! anything else the `image` crate can decode (GIF, BMP) is converted to PNG first, because
//! the model only stores encodings that DOCX, ODT and RTF can all embed.

use std::io::Cursor;
use std::sync::Arc;

use libertum_model::{Media, MediaError};

/// Why image bytes could not be turned into [`Media`].
#[derive(Debug, thiserror::Error)]
pub enum MediaLoadError {
    #[error("the image could not be decoded: {0}")]
    Decode(String),
    #[error(transparent)]
    Media(#[from] MediaError),
}

/// Wraps image bytes for the model. PNG and JPEG are kept byte for byte; other decodable
/// formats are re-encoded as PNG.
pub fn media_from_bytes(bytes: Vec<u8>) -> Result<Arc<Media>, MediaLoadError> {
    match Media::new(bytes.clone()) {
        Ok(media) => Ok(Arc::new(media)),
        Err(MediaError::UnknownFormat) => {
            let decoded = image::load_from_memory(&bytes)
                .map_err(|error| MediaLoadError::Decode(error.to_string()))?;
            let mut png = Vec::new();
            decoded
                .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
                .map_err(|error| MediaLoadError::Decode(error.to_string()))?;
            Ok(Arc::new(Media::new(png)?))
        }
        Err(error) => Err(error.into()),
    }
}

/// Encodes straight RGBA pixels as a PNG [`Media`] (tests, fixtures and examples).
pub fn png_from_rgba(width: u32, height: u32, rgba: Vec<u8>) -> Result<Arc<Media>, MediaLoadError> {
    let buffer = image::RgbaImage::from_raw(width, height, rgba)
        .ok_or_else(|| MediaLoadError::Decode("pixel buffer size mismatch".to_owned()))?;
    let mut png = Vec::new();
    buffer
        .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
        .map_err(|error| MediaLoadError::Decode(error.to_string()))?;
    Ok(Arc::new(Media::new(png)?))
}

/// A small real PNG (a two-colour checker) for tests and samples.
pub fn sample_png(width: u32, height: u32) -> Result<Arc<Media>, MediaLoadError> {
    let (width, height) = (width.max(1), height.max(1));
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for y in 0..height {
        for x in 0..width {
            let on = ((x / 4) + (y / 4)) % 2 == 0;
            rgba.extend_from_slice(if on {
                &[40, 110, 200, 255]
            } else {
                &[240, 240, 240, 255]
            });
        }
    }
    png_from_rgba(width, height, rgba)
}

#[cfg(test)]
mod tests {
    use super::*;
    use libertum_model::ImageFormat;

    #[test]
    fn png_passes_through_and_gif_becomes_png() {
        let png = sample_png(8, 6).expect("sample");
        assert_eq!(png.format(), ImageFormat::Png);
        assert_eq!((png.width_px(), png.height_px()), (8, 6));
        let again = media_from_bytes(png.bytes().to_vec()).expect("png");
        assert_eq!(again.bytes(), png.bytes(), "PNG bytes are kept as they are");

        let mut gif = Vec::new();
        image::RgbaImage::from_pixel(3, 2, image::Rgba([255, 0, 0, 255]))
            .write_to(&mut Cursor::new(&mut gif), image::ImageFormat::Gif)
            .expect("encode gif");
        let converted = media_from_bytes(gif).expect("gif converts");
        assert_eq!(converted.format(), ImageFormat::Png);
        assert_eq!((converted.width_px(), converted.height_px()), (3, 2));
    }

    #[test]
    fn rubbish_is_an_error_not_a_panic() {
        assert!(media_from_bytes(b"not an image".to_vec()).is_err());
        assert!(media_from_bytes(Vec::new()).is_err());
    }
}
