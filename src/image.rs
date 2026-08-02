//! Image conversion: JPEG ↔ PNG ↔ WebP ↔ AVIF ↔ JXL
//!
//! Decode path:
//!   JPEG  → zune-jpeg (pure Rust)
//!   PNG   → zune-png  (pure Rust)
//!   WebP  → image crate (pure Rust)
//!   AVIF  → image crate with `avif` feature (links dav1d via vcpkg)
//!   JXL   → jxl-oxide (pure Rust)
//!
//! Encode path:
//!   JPEG  → image crate PNG encoder (pure Rust)
//!   PNG   → image crate (pure Rust)
//!   WebP  → image crate lossless only (pure Rust; 0.25 has no lossy API)
//!   AVIF  → ravif (pure Rust rav1e encoder)
//!   JXL   → jpegxl-rs (libjxl via cmake, handled by jpegxl-src)

use crate::cli::ImageArgs;
use anyhow::{bail, Context, Result};
use std::path::Path;

// ─── format detection ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fmt { Jpeg, Png, WebP, Avif, Jxl }

impl Fmt {
    fn from_path(p: &Path) -> Result<Self> {
        match p.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref()
        {
            Some("jpg" | "jpeg") => Ok(Self::Jpeg),
            Some("png")          => Ok(Self::Png),
            Some("webp")         => Ok(Self::WebP),
            Some("avif")         => Ok(Self::Avif),
            Some("jxl")          => Ok(Self::Jxl),
            other => bail!("Unknown image extension: {:?}", other),
        }
    }
}

// ─── intermediate: RGBA8 ─────────────────────────────────────────────────────

struct Rgba8 {
    w:    u32,
    h:    u32,
    data: Vec<u8>, // interleaved RGBA, len = w * h * 4
}

impl Rgba8 {
    fn resize(&self, mut tw: u32, mut th: u32) -> Rgba8 {
        if tw == 0 && th == 0 { panic!("resize: both dimensions zero"); }
        if tw == 0 { tw = ((self.w as f64) * (th as f64) / (self.h as f64)).round() as u32; }
        if th == 0 { th = ((self.h as f64) * (tw as f64) / (self.w as f64)).round() as u32; }
        let img = image::RgbaImage::from_raw(self.w, self.h, self.data.clone())
            .expect("Rgba8::resize: bad buffer");
        let resized = image::imageops::resize(
            &img, tw, th, image::imageops::FilterType::Lanczos3,
        );
        Rgba8 { w: tw, h: th, data: resized.into_raw() }
    }

    fn rgb(&self) -> Vec<u8> {
        self.data.chunks_exact(4).flat_map(|p| [p[0], p[1], p[2]]).collect()
    }
}

// ─── decode ──────────────────────────────────────────────────────────────────

fn decode(path: &Path, fmt: Fmt) -> Result<Rgba8> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("reading {}", path.display()))?;
    match fmt {
        Fmt::Jpeg => decode_jpeg(&bytes),
        Fmt::Png  => decode_png(&bytes),
        Fmt::WebP => decode_via_image(&bytes, "WebP"),
        Fmt::Avif => decode_via_image(&bytes, "AVIF"),
        Fmt::Jxl  => decode_jxl(&bytes),
    }
}

fn decode_jpeg(bytes: &[u8]) -> Result<Rgba8> {
    use zune_jpeg::JpegDecoder;
    use zune_core::colorspace::ColorSpace;
    use zune_core::options::DecoderOptions;

    let opts = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA);
    let mut dec = JpegDecoder::new_with_options(bytes, opts);
    dec.decode_headers().context("JPEG header")?;
    let info = dec.info().context("JPEG info")?;
    let pixels = dec.decode().context("JPEG decode")?;
    Ok(Rgba8 { w: info.width as u32, h: info.height as u32, data: pixels })
}

fn decode_png(bytes: &[u8]) -> Result<Rgba8> {
    use zune_png::PngDecoder;
    use zune_core::options::DecoderOptions;

    let opts = DecoderOptions::default().png_set_add_alpha_channel(true);
    let mut dec = PngDecoder::new_with_options(bytes, opts);
    let pixels = dec.decode_raw().context("PNG decode")?;
    let info = dec.get_info().context("PNG info")?;
    Ok(Rgba8 { w: info.width as u32, h: info.height as u32, data: pixels })
}

fn decode_via_image(bytes: &[u8], label: &str) -> Result<Rgba8> {
    let img = image::load_from_memory(bytes)
        .with_context(|| format!("{label} decode"))?
        .to_rgba8();
    let (w, h) = img.dimensions();
    Ok(Rgba8 { w, h, data: img.into_raw() })
}

fn decode_jxl(bytes: &[u8]) -> Result<Rgba8> {
    use jxl_oxide::JxlImage;

    let mut image = JxlImage::read_with_defaults(std::io::Cursor::new(bytes))
        .map_err(|e| anyhow::anyhow!("JXL decode: {e}"))?;

    let render = image.render_frame(0).map_err(|e| anyhow::anyhow!("JXL render: {e}"))?;
    let fb = render.image_all_channels();
    let w = fb.width();
    let h = fb.height();
    let channels = fb.channels() as usize;
    let f32_buf = fb.buf();
    let mut rgba = Vec::with_capacity(w as usize * h as usize * 4);

    for pixel_idx in 0..(w as usize * h as usize) {
        let base = pixel_idx * channels;
        let r = (f32_buf[base].clamp(0.0, 1.0) * 255.0).round() as u8;
        let g = if channels > 1 { (f32_buf[base+1].clamp(0.0, 1.0) * 255.0).round() as u8 } else { r };
        let b = if channels > 2 { (f32_buf[base+2].clamp(0.0, 1.0) * 255.0).round() as u8 } else { r };
        let a = if channels > 3 { (f32_buf[base+3].clamp(0.0, 1.0) * 255.0).round() as u8 } else { 255 };
        rgba.extend_from_slice(&[r, g, b, a]);
    }

    Ok(Rgba8 { w, h, data: rgba })
}

// ─── encode ──────────────────────────────────────────────────────────────────

fn encode(img: &Rgba8, path: &Path, fmt: Fmt, quality: u8, lossless: bool) -> Result<()> {
    let bytes: Vec<u8> = match fmt {
        Fmt::Jpeg => encode_jpeg(img, quality)?,
        Fmt::Png  => encode_png(img)?,
        Fmt::WebP => encode_webp(img)?,
        Fmt::Avif => encode_avif(img, quality)?,
        Fmt::Jxl  => encode_jxl(img, quality, lossless)?,
    };
    std::fs::write(path, &bytes)
        .with_context(|| format!("writing {}", path.display()))
}

fn encode_jpeg(img: &Rgba8, quality: u8) -> Result<Vec<u8>> {
    // image crate JPEG encoder; zune-jpeg 0.4 only has a decoder
    let rgb = image::RgbImage::from_raw(
        img.w, img.h,
        img.rgb(),
    ).context("JPEG: bad pixel buffer")?;
    let mut out = std::io::Cursor::new(Vec::new());
    rgb.write_to(&mut out, image::ImageFormat::Jpeg)
        .context("JPEG encode")?;
    let _ = quality; // image 0.25 JPEG encoder uses default quality; quality param reserved
    Ok(out.into_inner())
}

fn encode_png(img: &Rgba8) -> Result<Vec<u8>> {
    let rgba = image::RgbaImage::from_raw(img.w, img.h, img.data.clone())
        .context("PNG: bad pixel buffer")?;
    let mut out = std::io::Cursor::new(Vec::new());
    rgba.write_to(&mut out, image::ImageFormat::Png)
        .context("PNG encode")?;
    Ok(out.into_inner())
}

fn encode_webp(img: &Rgba8) -> Result<Vec<u8>> {
    // image 0.25 WebPEncoder only exposes new_lossless; no lossy API
    let rgba = image::RgbaImage::from_raw(img.w, img.h, img.data.clone())
        .context("WebP: bad pixel buffer")?;
    let mut out = std::io::Cursor::new(Vec::new());
    let enc = image::codecs::webp::WebPEncoder::new_lossless(&mut out);
    enc.encode(
        &img.data, img.w, img.h,
        image::ExtendedColorType::Rgba8,
    ).context("WebP encode")?;
    Ok(out.into_inner())
}

fn encode_avif(img: &Rgba8, quality: u8) -> Result<Vec<u8>> {
    use ravif::{Encoder, Img, RGBA8};

    let ravif_quality = quality as f32;
    let pixels: &[RGBA8] = bytemuck::cast_slice(&img.data);
    let img_ref = Img::new(pixels, img.w as usize, img.h as usize);

    let encoded = Encoder::new()
        .with_quality(ravif_quality)
        .with_speed(4)
        .encode_rgba(img_ref)
        .context("AVIF encode")?;

    Ok(encoded.avif_file)
}

fn encode_jxl(img: &Rgba8, quality: u8, lossless: bool) -> Result<Vec<u8>> {
    use jpegxl_rs::encode::{EncoderSpeed, JxlEncoder};
    use jpegxl_rs::encoder_builder;

    let distance = if lossless { 0.0 } else { (100 - quality as u32) as f32 * 15.0 / 99.0 };

    let mut encoder = encoder_builder()
        .has_alpha(true)
        .lossless(Some(lossless))
        .speed(EncoderSpeed::Squirrel)
        .quality(distance)
        .build()
        .context("JXL encoder build")?;

    let result = encoder
        .encode::<u8, u8>(&img.data, img.w, img.h)
        .context("JXL encode")?;

    Ok(result.data)
}

// ─── resize helper ───────────────────────────────────────────────────────────

fn parse_resize(s: &str) -> Result<(u32, u32)> {
    let parts: Vec<&str> = s.splitn(2, 'x').collect();
    if parts.len() != 2 {
        bail!("--resize must be WIDTHxHEIGHT, e.g. 1920x1080 or 1920x0");
    }
    let w = parts[0].parse::<u32>().context("invalid width")?;
    let h = parts[1].parse::<u32>().context("invalid height")?;
    if w == 0 && h == 0 {
        bail!("--resize: at least one dimension must be non-zero");
    }
    Ok((w, h))
}

// ─── public entry point ──────────────────────────────────────────────────────

pub fn run(args: ImageArgs) -> Result<()> {
    let src_fmt = Fmt::from_path(&args.input)?;
    let dst_fmt = Fmt::from_path(&args.output)?;

    log::info!("decode {:?}  {}", src_fmt, args.input.display());
    let mut img = decode(&args.input, src_fmt)?;
    log::info!("decoded {}×{}", img.w, img.h);

    if let Some(ref dim) = args.resize {
        let (tw, th) = parse_resize(dim)?;
        img = img.resize(tw, th);
        log::info!("resized → {}×{}", img.w, img.h);
    }

    log::info!("encode  {:?}  {}", dst_fmt, args.output.display());
    encode(&img, &args.output, dst_fmt, args.quality, args.lossless)?;

    println!(
        "{} → {}  ({}×{})",
        args.input.display(), args.output.display(), img.w, img.h
    );
    Ok(())
}
