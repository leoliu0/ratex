//! Small repeatable benchmark for PNG-to-PDF image embedding.
//!
//! Run with:
//! `cargo run --release -p tex-core --example png_embed_bench -- image.png [speed|size|legacy] [level] [iterations]`

use flate2::write::ZlibEncoder;
use flate2::Compression;
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tex_core::pdf_images::{embed_png_with_options, PngEmbedOptions};

fn legacy_rgba_size(input: &[u8]) -> Result<(usize, usize), String> {
    let mut decoder = png::Decoder::new(std::io::Cursor::new(input));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder
        .read_info()
        .map_err(|error| format!("invalid PNG: {error}"))?;
    let mut pixels = vec![0; reader.output_buffer_size().ok_or("PNG is too large")?];
    let info = reader
        .next_frame(&mut pixels)
        .map_err(|error| format!("cannot decode PNG: {error}"))?;
    if info.color_type != png::ColorType::Rgba || info.bit_depth != png::BitDepth::Eight {
        return Err("legacy benchmark supports 8-bit RGBA PNGs only".into());
    }
    pixels.truncate(info.buffer_size());
    let width = usize::try_from(info.width).map_err(|_| "image width does not fit usize")?;
    let mut rgb = ZlibEncoder::new(Vec::new(), Compression::fast());
    let mut alpha = ZlibEncoder::new(Vec::new(), Compression::fast());
    let mut rgb_row = vec![0; 1 + width * 3];
    let mut alpha_row = vec![0; 1 + width];
    for row in pixels.chunks_exact(width * 4) {
        for pixel in 0..width {
            let offset = pixel * 4;
            rgb_row[1 + pixel * 3..1 + pixel * 3 + 3].copy_from_slice(&row[offset..offset + 3]);
            alpha_row[1 + pixel] = row[offset + 3];
        }
        rgb.write_all(&rgb_row).map_err(|error| error.to_string())?;
        alpha
            .write_all(&alpha_row)
            .map_err(|error| error.to_string())?;
    }
    let rgb = rgb.finish().map_err(|error| error.to_string())?;
    let alpha = alpha.finish().map_err(|error| error.to_string())?;
    // Include the exact dictionaries and delimiters used by the old code so
    // `object_bytes` remains comparable with current modes.
    let smask_header = format!(
        "<< /Type /XObject /Subtype /Image /Width {} /Height {} /BitsPerComponent 8 /ColorSpace /DeviceGray /Filter /FlateDecode /DecodeParms << /Predictor 15 /Columns {} /Colors 1 /BitsPerComponent 8 >> /Length {} >>\nstream\n",
        info.width,
        info.height,
        info.width,
        alpha.len()
    );
    let image_header = format!(
        "<< /Type /XObject /Subtype /Image /Width {} /Height {} /BitsPerComponent 8 /ColorSpace /DeviceRGB /SMask 2 0 R /Filter /FlateDecode /DecodeParms << /Predictor 15 /Columns {} /Colors 3 /BitsPerComponent 8 >> /Length {} >>\nstream\n",
        info.width,
        info.height,
        info.width,
        rgb.len()
    );
    Ok((
        smask_header.len()
            + alpha.len()
            + b"\nendstream".len()
            + image_header.len()
            + rgb.len()
            + b"\nendstream".len(),
        2,
    ))
}

fn main() -> Result<(), String> {
    let mut args = std::env::args_os().skip(1);
    let path = PathBuf::from(
        args.next()
            .ok_or("usage: png_embed_bench IMAGE.png [speed|size|legacy] [level] [iterations]")?,
    );
    let mode = args.next().unwrap_or_else(|| "speed".into());
    let level = args
        .next()
        .map(|value| value.to_string_lossy().parse::<u32>())
        .transpose()
        .map_err(|error| format!("invalid compression level: {error}"))?
        .unwrap_or(3);
    let iterations = args
        .next()
        .map(|value| value.to_string_lossy().parse::<u32>())
        .transpose()
        .map_err(|error| format!("invalid iteration count: {error}"))?
        .unwrap_or(3)
        .max(1);
    if args.next().is_some() {
        return Err("too many arguments".into());
    }
    let mode_name = mode.to_string_lossy();
    let options = match mode_name.as_ref() {
        "speed" => Some(PngEmbedOptions::speed(level)),
        "size" => Some(PngEmbedOptions::size(level)),
        "legacy" => None,
        value => {
            return Err(format!(
                "unknown mode `{value}`; expected speed, size, or legacy"
            ));
        }
    };
    let input =
        std::fs::read(&path).map_err(|error| format!("cannot read {}: {error}", path.display()))?;
    let mut elapsed = Duration::ZERO;
    let mut output_bytes = 0;
    let mut object_count = 0;
    for _ in 0..iterations {
        let start = Instant::now();
        if let Some(options) = options {
            let mut next_object = 2;
            let objects = embed_png_with_options(&input, 1, &mut next_object, options)
                .ok_or_else(|| format!("cannot embed {}", path.display()))?;
            output_bytes = objects.iter().map(|object| object.bytes.len()).sum();
            object_count = objects.len();
        } else {
            (output_bytes, object_count) = legacy_rgba_size(&input)?;
        }
        elapsed += start.elapsed();
    }
    println!(
        "mode={} level={} iterations={} input_bytes={} object_bytes={} objects={} mean_ms={:.3}",
        mode_name,
        if options.is_some() { level.min(9) } else { 1 },
        iterations,
        input.len(),
        output_bytes,
        object_count,
        elapsed.as_secs_f64() * 1_000.0 / f64::from(iterations),
    );
    Ok(())
}
