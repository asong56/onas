use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// onas — image conversion and audio transcoding CLI
#[derive(Parser)]
#[command(
    name = "onas", version, about, long_about = None,
    arg_required_else_help = true,
    after_help = "\
Formats:
  image  JPEG, PNG, WebP, AVIF, JXL
  audio  FLAC, Opus, M4A/AAC

Examples:
  onas image photo.avif photo.webp -q 85
  onas audio track.flac track.opus
  onas meta song.mp3 --dump"
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    /// Also print a machine-readable JSON summary (or error) as the last
    /// line of output, for callers driving onas as a subprocess. See
    /// exit codes in `onas --help` output / README for the paired
    /// process exit-status contract.
    #[arg(long, global = true)]
    pub json: bool,
}

#[derive(Subcommand)]
pub enum Command {
    /// Convert between image formats (JPEG ↔ PNG ↔ WebP ↔ AVIF ↔ JXL)
    Image(ImageArgs),
    /// Transcode audio files (FLAC ↔ Opus ↔ M4A)
    Audio(AudioArgs),
    /// Read or edit file metadata (EXIF for images, tags for audio)
    Meta(MetaArgs),
}

// ─── Image ───────────────────────────────────────────────────────────────────

#[derive(Args)]
pub struct ImageArgs {
    /// Input file
    pub input: PathBuf,
    /// Output file — extension determines target format
    pub output: PathBuf,

    /// Encode quality 1–100 (JPEG / WebP lossy / AVIF)
    #[arg(short, long, default_value_t = 90)]
    pub quality: u8,

    /// Force lossless encoding (WebP, JXL)
    #[arg(long)]
    pub lossless: bool,

    /// Resize: WIDTHxHEIGHT — use 0 for one dimension to keep aspect ratio
    /// e.g. --resize 1920x0 or --resize 0x1080
    #[arg(long, value_name = "WxH")]
    pub resize: Option<String>,
}

// ─── Audio ───────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, ValueEnum)]
pub enum AudioFmt {
    Flac,
    Opus,
    /// AAC wrapped in an M4A container
    M4a,
}

#[derive(Args)]
pub struct AudioArgs {
    /// Input file (FLAC, Opus, M4A/AAC, MP3, OGG, WAV)
    pub input: PathBuf,
    /// Output file — extension sets format automatically,
    /// or override with --format
    pub output: PathBuf,

    /// Force output format (auto-detected from extension by default)
    #[arg(short, long, value_enum)]
    pub format: Option<AudioFmt>,

    /// Bitrate in kbps for lossy codecs (Opus, AAC)
    #[arg(short, long, default_value_t = 192)]
    pub bitrate: u32,

    /// FLAC compression level 0–8 (0 = fastest, 8 = smallest)
    #[arg(long, default_value_t = 5)]
    pub compression: u8,
}

impl AudioArgs {
    /// Resolve the target format from --format flag or output file extension.
    pub fn target_fmt(&self) -> anyhow::Result<AudioFmt> {
        if let Some(ref f) = self.format {
            return Ok(f.clone());
        }
        match self.output.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref()
        {
            Some("flac")        => Ok(AudioFmt::Flac),
            Some("opus")        => Ok(AudioFmt::Opus),
            Some("m4a" | "aac") => Ok(AudioFmt::M4a),
            other => anyhow::bail!(
                "Cannot detect audio format from extension {:?}; use --format",
                other
            ),
        }
    }
}

// ─── Metadata ────────────────────────────────────────────────────────────────

#[derive(Args)]
pub struct MetaArgs {
    /// File to inspect or edit (image or audio)
    pub file: PathBuf,

    /// Print all metadata and exit (read-only)
    #[arg(long)]
    pub dump: bool,

    /// Set a tag field: KEY=VALUE  (repeatable)
    /// For images this writes the EXIF UserComment / Description.
    /// For audio this writes standard tag fields (Title, Artist, Album, …)
    #[arg(long = "set", value_name = "KEY=VALUE", num_args = 1)]
    pub set: Vec<String>,

    /// Remove a tag field by key  (repeatable)
    #[arg(long = "remove", value_name = "KEY", num_args = 1)]
    pub remove: Vec<String>,

    /// Strip ALL metadata from the file
    #[arg(long)]
    pub strip: bool,

    /// Embed a cover image (front cover) from this file — audio only
    #[arg(long, value_name = "IMAGE_PATH")]
    pub cover: Option<PathBuf>,

    /// Extract the embedded cover image to this file — audio only, read-only
    #[arg(long, value_name = "OUT_PATH")]
    pub extract_cover: Option<PathBuf>,
}

