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
    use nom_exif::{MediaParser, MediaSource, Exif};

    let ms = MediaSource::file_path(path)
        .with_context(|| format!("opening {}", path.display()))?;
    let mut parser = MediaParser::new();

    match parser.parse(ms) {
        Ok(nom_exif::Metadata::Exif(exif)) => {
            let iter = exif.iter();
            let mut found = false;
            for (tag, entry) in iter {
                println!("{:?}\t{}", tag, entry);
                found = true;
            }
            if !found {
                println!("(no EXIF data)");
            }
        }
        Ok(nom_exif::Metadata::Track(track)) => {
            println!("Duration: {:?}", track.get(nom_exif::TrackInfoTag::Duration));
            println!("Width:    {:?}", track.get(nom_exif::TrackInfoTag::ImageWidth));
            println!("Height:   {:?}", track.get(nom_exif::TrackInfoTag::ImageHeight));
        }
        Err(e) => {
            println!("(no metadata or unsupported format: {e})");
        }
    }
    Ok(())
}

// ─── image: write via lofty ──────────────────────────────────────────────────

fn image_write(path: &Path, sets: &[String], removes: &[String], strip: bool) -> Result<()> {
    // lofty supports JPEG and PNG tag writing (EXIF / XMP / iTXt chunks)
    use lofty::{
        file::TaggedFileExt,
        prelude::{AudioFile, TagExt},
        probe::Probe,
        tag::{Tag, TagType, ItemKey, ItemValue, TagItem},
        config::WriteOptions,
    };

    let mut tagged = Probe::open(path)
        .with_context(|| format!("lofty open {}", path.display()))?
        .guess_file_type()
        .context("lofty guess file type")?
        .read()
        .context("lofty read")?;

    if strip {
        for tag in tagged.tags_mut() {
            tag.clear();
        }
        tagged.save().context("lofty save (strip)")?;
        println!("Stripped all metadata from {}", path.display());
        return Ok(());
    }

    // Use first available tag, or create an ID3v2 one
    let tag_type = tagged.primary_tag_type();
    if tagged.primary_tag().is_none() {
        tagged.insert_tag(Tag::new(tag_type));
    }

    let tag = tagged.primary_tag_mut().unwrap();

    for r in removes {
        // Try known ItemKey first, fall back to custom key removal
        match ItemKey::from_key(tag.tag_type(), r) {
            Some(k) => { tag.remove_key(k); }
            None    => { tag.remove_key(ItemKey::Unknown(r.to_owned())); }
        }
        println!("Removed: {r}");
    }

    for kv in sets {
        let (k, v) = kv.split_once('=')
            .with_context(|| format!("--set value must be KEY=VALUE, got: {kv}"))?;
        let item_key = ItemKey::from_key(tag.tag_type(), k)
            .unwrap_or_else(|| ItemKey::Unknown(k.to_owned()));
        tag.insert_text(item_key, v.to_owned());
        println!("Set: {k} = {v}");
    }

    tagged.save().context("lofty save")?;
    Ok(())
}

// ─── audio: dump tags ────────────────────────────────────────────────────────

fn audio_dump(path: &Path) -> Result<()> {
    use lofty::{prelude::AudioFile, probe::Probe};

    let tagged = Probe::open(path)
        .with_context(|| format!("opening {}", path.display()))?
        .guess_file_type()
        .context("guess file type")?
        .read()
        .context("read file")?;

    // Print audio properties
    if let Some(props) = tagged.properties() {
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
    }

    // Print all tags
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
        file::TaggedFileExt,
        prelude::{AudioFile, TagExt},
        probe::Probe,
        tag::{Tag, ItemKey, ItemValue, TagItem},
    };

    let mut tagged = Probe::open(path)
        .with_context(|| format!("opening {}", path.display()))?
        .guess_file_type()
        .context("guess file type")?
        .read()
        .context("read file")?;

    if strip {
        for tag in tagged.tags_mut() {
            tag.clear();
        }
        tagged.save().context("save (strip)")?;
        println!("Stripped all tags from {}", path.display());
        return Ok(());
    }

    let tag_type = tagged.primary_tag_type();
    if tagged.primary_tag().is_none() {
        tagged.insert_tag(Tag::new(tag_type));
    }

    let tag = tagged.primary_tag_mut().unwrap();

    for r in removes {
        match ItemKey::from_key(tag.tag_type(), r) {
            Some(k) => { tag.remove_key(k); }
            None    => { tag.remove_key(ItemKey::Unknown(r.to_owned())); }
        }
        println!("Removed: {r}");
    }

    for kv in sets {
        let (k, v) = kv.split_once('=')
            .with_context(|| format!("--set must be KEY=VALUE, got: {kv}"))?;
        let item_key = ItemKey::from_key(tag.tag_type(), k)
            .unwrap_or_else(|| ItemKey::Unknown(k.to_owned()));
        tag.insert_text(item_key, v.to_owned());
        println!("Set: {k} = {v}");
    }

    tagged.save().context("save")?;
    Ok(())
}

// ─── public entry point ──────────────────────────────────────────────────────

pub fn run(args: MetaArgs) -> Result<()> {
    let class = classify(&args.file)?;

    // Validate mutually exclusive write flags
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
