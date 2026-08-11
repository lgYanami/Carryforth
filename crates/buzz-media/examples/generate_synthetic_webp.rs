//! Generate the shared WebP upload fixture from a fixed synthetic pixel grid.

use std::{env, error::Error, fs::File, io::BufWriter, path::PathBuf};

use image::{codecs::webp::WebPEncoder, ExtendedColorType};

fn main() -> Result<(), Box<dyn Error>> {
    let output = env::args_os().nth(1).map(PathBuf::from).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "usage: generate_synthetic_webp <output.webp>",
        )
    })?;

    // Two red pixels followed by two green pixels. The input is generated data,
    // contains no external artwork, and matches pixel-grid-2x2.png.
    let pixels = [255, 0, 0, 255, 0, 0, 0, 255, 0, 0, 255, 0];
    let writer = BufWriter::new(File::create(output)?);
    WebPEncoder::new_lossless(writer).encode(&pixels, 2, 2, ExtendedColorType::Rgb8)?;
    Ok(())
}
