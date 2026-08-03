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
        Some("jpg" | "jpeg" | "png" | "webp" | "avif" | "jxl") => Ok(Class::Image),
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
            let mut found = false;
            for entry in exif_iter {
                println!("{:?}\t{}", entry.tag(), entry.value().unwrap_or(&nom_exif::EntryValue::Text(String::new())));
                found = true;
            }
            if !found {
                println!("(no EXIF data)");
            }
        }
        Err(_) => {
            // Try as track/video metadata
            let ms2 = MediaSource::open(path)
                .with_context(|| format!("opening {}", path.display()))?;
            match parser.parse_track(ms2) {
                Ok(track) => {
                    println!("Duration: {:?}", track.get(nom_exif::TrackInfoTag::DurationMs));
                    println!("Width:    {:?}", track.get(nom_exif::TrackInfoTag::Width));
                    println!("Height:   {:?}", track.get(nom_exif::TrackInfoTag::Height));
                }
                Err(e) => {
                    println!("(no metadata or unsupported format: {e})");
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
        println!("\n=== {:?} ===", tag.tag_type());
        for item in tag.items() {
            println!("{:?}\t{:?}", item.key(), item.value());
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

// ─── public entry point ──────────────────────────────────────────────────────

pub fn run(args: MetaArgs) -> Result<()> {
    let class = classify(&args.file)?;

    if args.strip && (!args.set.is_empty() || !args.remove.is_empty()) {
        bail!("--strip cannot be combined with --set or --remove");
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
