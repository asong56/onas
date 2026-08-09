//! Metadata read / write.
//!
//! Images: nom-exif 3 for reading EXIF; lofty for writing (via JPEG APP1 rewrite)
//! Audio:  lofty for reading and writing all tag formats
//!         (ID3v2 for MP3, VorbisComments for FLAC/Opus, MP4 ilst for M4A)

use crate::cli::MetaArgs;
use anyhow::{bail, Context, Result};
use std::path::Path;

// ─── file class ──────────────────────────────────────────────────────────────

enum Class { Image, Audio }

fn classify(p: &Path) -> Result<Class> {
    match p.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        // nom-exif also parses container track metadata (falls through to
        // parse_track in image_dump below), so route Matroska/ISOBMFF video
        // containers through the same path instead of rejecting them.
        Some("jpg" | "jpeg" | "png" | "webp" | "avif" | "jxl"
             | "mkv" | "webm" | "mka" | "mp4" | "mov" | "3gp") => Ok(Class::Image),
        Some("flac" | "opus" | "m4a" | "aac" | "mp3" | "ogg" | "wav") => Ok(Class::Audio),
        other => bail!("Cannot determine metadata class from extension: {:?}", other),
    }
}

// ─── image: dump EXIF ────────────────────────────────────────────────────────

fn image_dump(path: &Path) -> Result<()> {
    use nom_exif::{MediaParser, MediaSource};

    let ms = MediaSource::open(path)
        .with_context(|| format!("opening {}", path.display()))?;
    let mut parser = MediaParser::new();

    let iter = parser.parse_exif(ms);
    match iter {
        Ok(exif_iter) => {
            let mut rows: Vec<(String, String)> = Vec::new();
            for entry in exif_iter {
                let value = entry.value()
                    .map(|v| v.to_string())
                    .unwrap_or_default();
                rows.push((format!("{:?}", entry.tag()), value));
            }
            if rows.is_empty() {
                println!("(no EXIF data)");
            } else {
                let width = rows.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
                println!("=== EXIF ===");
                for (k, v) in rows {
                    println!("{k:width$}  {v}");
                }
            }
        }
        Err(e) => {
            log::debug!("parse_exif failed: {e}");
            // Try as track/video metadata
            let ms2 = MediaSource::open(path)
                .with_context(|| format!("opening {}", path.display()))?;
            match parser.parse_track(ms2) {
                Ok(track) => {
                    // EntryValue implements Display; format through that
                    // instead of {:?} so this prints "1920" rather than
                    // the doubly-wrapped "Some(U32(1920))".
                    let field = |tag| match track.get(tag) {
                        Some(v) => v.to_string(),
                        None    => "(unknown)".to_string(),
                    };
                    println!("Duration: {} ms", field(nom_exif::TrackInfoTag::DurationMs));
                    println!("Width:    {}",    field(nom_exif::TrackInfoTag::Width));
                    println!("Height:   {}",    field(nom_exif::TrackInfoTag::Height));
                }
                Err(e) => {
                    log::debug!("parse_track failed: {e}");
                    println!("No metadata found (no EXIF data and no track info in this file).");
                }
            }
        }
    }
    Ok(())
}

// ─── image: write via lofty ──────────────────────────────────────────────────

fn image_write(path: &Path, sets: &[String], removes: &[String], strip: bool) -> Result<()> {
    use lofty::{
        config::WriteOptions,
        file::{AudioFile, TaggedFileExt},
        prelude::TagExt,
        probe::Probe,
        tag::{ItemKey, Tag},
    };

    let mut tagged = Probe::open(path)
        .with_context(|| format!("lofty open {}", path.display()))?
        .guess_file_type()
        .context("lofty guess file type")?
        .read()
        .context("lofty read")?;

    if strip {
        for tag_type in tagged.tags().iter().map(|t| t.tag_type()).collect::<Vec<_>>() {
            if let Some(t) = tagged.tag_mut(tag_type) {
                t.clear();
            }
        }
        tagged.save_to_path(path, WriteOptions::default()).context("lofty save (strip)")?;
        println!("Stripped all metadata from {}", path.display());
        return Ok(());
    }

    let tag_type = tagged.primary_tag_type();
    if tagged.primary_tag().is_none() {
        tagged.insert_tag(Tag::new(tag_type));
    }

    let tag = tagged.primary_tag_mut().unwrap();

    for r in removes {
        // lofty 0.24's ItemKey is a closed (non-exhaustive but fixed) set of
        // well-known keys — there's no catch-all variant for arbitrary
        // strings, so a key this tag format doesn't recognize simply can't
        // be removed by key. Skip it rather than silently pretending it
        // worked.
        match ItemKey::from_key(tag.tag_type(), r) {
            Some(k) => { tag.remove_key(k); println!("Removed: {r}"); }
            None    => println!("Skipped '{r}': not a recognized key for {:?}", tag.tag_type()),
        }
    }

    for kv in sets {
        let (k, v) = kv.split_once('=')
            .with_context(|| format!("--set value must be KEY=VALUE, got: {kv}"))?;
        let item_key = ItemKey::from_key(tag.tag_type(), k)
            .with_context(|| format!(
                "'{k}' is not a recognized metadata key for {:?}", tag.tag_type()
            ))?;
        tag.insert_text(item_key, v.to_owned());
        println!("Set: {k} = {v}");
    }

    tagged.save_to_path(path, WriteOptions::default()).context("lofty save")?;
    Ok(())
}

// ─── audio: dump tags ────────────────────────────────────────────────────────

fn audio_dump(path: &Path) -> Result<()> {
    use lofty::{file::TaggedFileExt, prelude::AudioFile, probe::Probe};

    let tagged = Probe::open(path)
        .with_context(|| format!("opening {}", path.display()))?
        .guess_file_type()
        .context("guess file type")?
        .read()
        .context("read file")?;

    let props = tagged.properties();
    println!("=== Audio Properties ===");
    println!("Duration:    {:.2}s", props.duration().as_secs_f64());
    if let Some(br) = props.audio_bitrate() {
        println!("Bitrate:     {} kbps", br);
    }
    if let Some(sr) = props.sample_rate() {
        println!("Sample rate: {} Hz", sr);
    }
    if let Some(ch) = props.channels() {
        println!("Channels:    {}", ch);
    }

    if tagged.tags().is_empty() {
        println!("(no tags)");
    }
    for tag in tagged.tags() {
        use lofty::prelude::Accessor;
        println!("\n=== {:?} tags ===", tag.tag_type());

        // Print the common fields as plain text via lofty's Accessor
        // trait, rather than the raw ItemKey/ItemValue enums — those
        // Debug-format as e.g. `TrackTitle` / `Text("Song Name")`, which
        // is Rust internals, not something a person asked to read.
        let mut common: Vec<(&str, String)> = Vec::new();
        if let Some(v) = tag.title()   { common.push(("Title",   v.into_owned())); }
        if let Some(v) = tag.artist()  { common.push(("Artist",  v.into_owned())); }
        if let Some(v) = tag.album()   { common.push(("Album",   v.into_owned())); }
        if let Some(v) = tag.genre()   { common.push(("Genre",   v.into_owned())); }
        if let Some(v) = tag.comment() { common.push(("Comment", v.into_owned())); }

        let width = common.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
        for (k, v) in &common {
            println!("{k:width$}  {v}");
        }

        // Anything else present that isn't one of the fields above —
        // listed by key name only, since there's no reliable way to
        // pretty-print an arbitrary ItemValue's contents as plain text.
        let common_keys = ["TrackTitle", "TrackArtist", "AlbumTitle", "Genre", "Comment", "Year", "TrackNumber"];
        let other: Vec<String> = tag.items()
            .map(|item| format!("{:?}", item.key()))
            .filter(|k| !common_keys.contains(&k.as_str()))
            .collect();
        if !other.is_empty() {
            println!("Other fields present: {}", other.join(", "));
        }
    }
    Ok(())
}

// ─── audio: write tags ───────────────────────────────────────────────────────

fn audio_write(path: &Path, sets: &[String], removes: &[String], strip: bool) -> Result<()> {
    use lofty::{
        config::WriteOptions,
        file::{AudioFile, TaggedFileExt},
        prelude::TagExt,
        probe::Probe,
        tag::{ItemKey, Tag},
    };

    let mut tagged = Probe::open(path)
        .with_context(|| format!("opening {}", path.display()))?
        .guess_file_type()
        .context("guess file type")?
        .read()
        .context("read file")?;

    if strip {
        for tag_type in tagged.tags().iter().map(|t| t.tag_type()).collect::<Vec<_>>() {
            if let Some(t) = tagged.tag_mut(tag_type) {
                t.clear();
            }
        }
        tagged.save_to_path(path, WriteOptions::default()).context("save (strip)")?;
        println!("Stripped all tags from {}", path.display());
        return Ok(());
    }

    let tag_type = tagged.primary_tag_type();
    if tagged.primary_tag().is_none() {
        tagged.insert_tag(Tag::new(tag_type));
    }

    let tag = tagged.primary_tag_mut().unwrap();

    for r in removes {
        // See image_write: lofty 0.24's ItemKey has no catch-all variant,
        // so a key this tag format doesn't recognize can't be targeted.
        match ItemKey::from_key(tag.tag_type(), r) {
            Some(k) => { tag.remove_key(k); println!("Removed: {r}"); }
            None    => println!("Skipped '{r}': not a recognized key for {:?}", tag.tag_type()),
        }
    }

    for kv in sets {
        let (k, v) = kv.split_once('=')
            .with_context(|| format!("--set must be KEY=VALUE, got: {kv}"))?;
        let item_key = ItemKey::from_key(tag.tag_type(), k)
            .with_context(|| format!(
                "'{k}' is not a recognized metadata key for {:?}", tag.tag_type()
            ))?;
        tag.insert_text(item_key, v.to_owned());
        println!("Set: {k} = {v}");
    }

    tagged.save_to_path(path, WriteOptions::default()).context("save")?;
    Ok(())
}

// ─── audio: cover art ────────────────────────────────────────────────────────
// lofty already reads/writes embedded pictures (APIC for ID3v2, METADATA_BLOCK_PICTURE
// for FLAC/Vorbis Comments, covr for MP4 ilst) — it's the same dependency audio_write
// above already uses, so this costs nothing extra to turn on.

fn audio_set_cover(path: &Path, image_path: &Path) -> Result<()> {
    use lofty::{
        config::WriteOptions,
        file::{AudioFile, TaggedFileExt},
        picture::{MimeType, Picture, PictureType},
        probe::Probe,
        tag::Tag,
    };

    let img_bytes = std::fs::read(image_path)
        .with_context(|| format!("reading {}", image_path.display()))?;
    let mime = match image_path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("png")          => MimeType::Png,
        Some("jpg" | "jpeg") => MimeType::Jpeg,
        Some("gif")          => MimeType::Gif,
        Some("bmp")          => MimeType::Bmp,
        Some("tif" | "tiff") => MimeType::Tiff,
        other => MimeType::Unknown(other.unwrap_or("application/octet-stream").to_owned()),
    };

    let mut tagged = Probe::open(path)
        .with_context(|| format!("opening {}", path.display()))?
        .guess_file_type()
        .context("guess file type")?
        .read()
        .context("read file")?;

    let tag_type = tagged.primary_tag_type();
    if tagged.primary_tag().is_none() {
        tagged.insert_tag(Tag::new(tag_type));
    }
    let tag = tagged.primary_tag_mut().unwrap();

    // Replace any existing front cover instead of stacking duplicates.
    let existing: Vec<usize> = tag.pictures().iter().enumerate()
        .filter(|(_, p)| p.pic_type() == PictureType::CoverFront)
        .map(|(i, _)| i)
        .collect();
    for i in existing.into_iter().rev() {
        tag.remove_picture(i);
    }
    tag.push_picture(
        Picture::unchecked(img_bytes)
            .pic_type(PictureType::CoverFront)
            .mime_type(mime)
            .build(),
    );

    tagged.save_to_path(path, WriteOptions::default()).context("save (cover)")?;
    println!("Embedded cover: {}", image_path.display());
    Ok(())
}

fn audio_extract_cover(path: &Path, out_path: &Path) -> Result<()> {
    use lofty::{file::TaggedFileExt, picture::PictureType, probe::Probe};

    let tagged = Probe::open(path)
        .with_context(|| format!("opening {}", path.display()))?
        .guess_file_type()
        .context("guess file type")?
        .read()
        .context("read file")?;

    let pic = tagged.tags().iter().flat_map(|t| t.pictures())
        .find(|p| p.pic_type() == PictureType::CoverFront)
        .or_else(|| tagged.tags().iter().flat_map(|t| t.pictures()).next())
        .context("no embedded cover picture found")?;

    std::fs::write(out_path, pic.data())
        .with_context(|| format!("writing {}", out_path.display()))?;
    println!("Extracted cover ({} bytes) → {}", pic.data().len(), out_path.display());
    Ok(())
}

// ─── public entry point ──────────────────────────────────────────────────────

pub fn run(args: MetaArgs) -> Result<()> {
    let class = classify(&args.file)?;

    if args.strip && (!args.set.is_empty() || !args.remove.is_empty()) {
        bail!("--strip cannot be combined with --set or --remove");
    }
    if let Some(out) = &args.extract_cover {
        return match class {
            Class::Audio => audio_extract_cover(&args.file, out),
            Class::Image => bail!("--extract-cover is audio-only"),
        };
    }
    if let Some(img) = &args.cover {
        return match class {
            Class::Audio => audio_set_cover(&args.file, img),
            Class::Image => bail!("--cover is audio-only"),
        };
    }
    let is_read_only = args.dump || (!args.strip && args.set.is_empty() && args.remove.is_empty());

    if is_read_only {
        match class {
            Class::Image => image_dump(&args.file),
            Class::Audio => audio_dump(&args.file),
        }
    } else {
        match class {
            Class::Image => image_write(&args.file, &args.set, &args.remove, args.strip),
            Class::Audio => audio_write(&args.file, &args.set, &args.remove, args.strip),
        }
    }
}
