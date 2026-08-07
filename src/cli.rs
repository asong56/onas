use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

/// onas — image conversion, audio and video transcoding CLI
#[derive(Parser)]
#[command(
    name = "onas", version, about, long_about = None,
    arg_required_else_help = true,
    after_help = "\
Formats:
  image  JPEG, PNG, WebP, AVIF, JXL
  audio  FLAC, Opus, M4A/AAC
  video  H.264, H.265, VP9, AV1  (MKV container)

Examples:
  onas image photo.avif photo.webp -q 85
  onas audio track.flac track.opus
  onas video in.mp4 out.mkv -v h265 --crf 20
  onas frame in.mkv thumb.png --at 12.5
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
    /// Transcode video files (H.264 ↔ H.265 ↔ VP9 ↔ AV1, MKV container)
    Video(VideoArgs),
    /// Extract a single still frame from a video as an image (PNG/JPEG)
    Frame(FrameArgs),
    /// Read or edit file metadata (EXIF for images, tags for audio/video)
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
    /// Input file (FLAC, Opus, M4A/AAC, MP3, OGG, WAV, …)
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

// ─── Frame extraction ────────────────────────────────────────────────────────

#[derive(Clone, Debug, ValueEnum)]
pub enum FrameFmt {
    Png,
    Jpeg,
}

#[derive(Args)]
pub struct FrameArgs {
    /// Input video file (MKV container)
    pub input: PathBuf,
    /// Output image file — extension determines format unless --format is given
    pub output: PathBuf,

    /// Seek target: either a timestamp (seconds, e.g. `12.5` or `1:02.5`)
    /// or a frame number, selected by --at-frame instead
    #[arg(long, value_name = "SECONDS", conflicts_with = "at_frame")]
    pub at: Option<String>,

    /// Seek target as a zero-based decoded-frame index instead of a timestamp
    #[arg(long, value_name = "N", conflicts_with = "at")]
    pub at_frame: Option<u64>,

    /// Force output image format (auto-detected from extension by default)
    #[arg(short, long, value_enum)]
    pub format: Option<FrameFmt>,

    /// JPEG/output quality 1–100 (ignored for lossless PNG)
    #[arg(short, long, default_value_t = 90)]
    pub quality: u8,

    /// Resize the extracted frame: WIDTHxHEIGHT — 0 keeps aspect ratio
    #[arg(long, value_name = "WxH")]
    pub resize: Option<String>,
}

impl FrameArgs {
    /// Resolve the target output image format from --format or extension.
    pub fn target_fmt(&self) -> anyhow::Result<FrameFmt> {
        if let Some(ref f) = self.format {
            return Ok(f.clone());
        }
        match self.output.extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .as_deref()
        {
            Some("png")         => Ok(FrameFmt::Png),
            Some("jpg" | "jpeg") => Ok(FrameFmt::Jpeg),
            other => anyhow::bail!(
                "Cannot detect image format from extension {:?}; use --format",
                other
            ),
        }
    }

    /// Parse `--at` into milliseconds, accepting either plain seconds
    /// (`12.5`) or `MM:SS.mmm` / `H:MM:SS.mmm` timecodes.
    pub fn at_ms(&self) -> anyhow::Result<Option<i64>> {
        let Some(ref s) = self.at else { return Ok(None) };
        if let Ok(secs) = s.parse::<f64>() {
            return Ok(Some((secs * 1000.0).round() as i64));
        }
        let parts: Vec<&str> = s.split(':').collect();
        if parts.is_empty() || parts.len() > 3 {
            anyhow::bail!("--at: expected SECONDS or [H:]MM:SS[.mmm], got {:?}", s);
        }
        let mut secs = 0f64;
        for p in &parts {
            let v: f64 = p.parse()
                .map_err(|_| anyhow::anyhow!("--at: invalid time component {:?}", p))?;
            secs = secs * 60.0 + v;
        }
        Ok(Some((secs * 1000.0).round() as i64))
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
}

// ─── Video ───────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, ValueEnum)]
pub enum VideoCodec {
    H264,
    H265,
    Vp9,
    Av1,
    /// Copy stream without re-encoding
    Copy,
}

#[derive(Clone, Debug, ValueEnum)]
pub enum VideoAudioCodec {
    /// Opus (recommended for MKV)
    Opus,
    /// AAC-LC
    Aac,
    /// FLAC (lossless)
    Flac,
    /// Copy audio stream without re-encoding
    Copy,
}

#[derive(Args)]
pub struct VideoArgs {
    /// Input file (any container FFmpeg can read: MKV, MP4, MOV, AVI, …)
    pub input: PathBuf,

    /// Output file — MUST be .mkv
    pub output: PathBuf,

    /// Video codec for output
    #[arg(short = 'v', long, default_value = "h264")]
    pub vcodec: VideoCodec,

    /// Audio codec for output
    #[arg(short = 'a', long, default_value = "opus")]
    pub acodec: VideoAudioCodec,

    /// CRF quality factor (lower = better; H.264: 0–51, H.265: 0–51,
    /// VP9: 0–63, AV1: 0–63). Default 23 suits H.264/H.265.
    #[arg(long, default_value_t = 23)]
    pub crf: u8,

    /// Maximum video bitrate in kbps (enables constrained-quality mode)
    #[arg(long)]
    pub vbitrate: Option<u32>,

    /// Audio bitrate in kbps for lossy codecs (Opus / AAC)
    #[arg(long, default_value_t = 192)]
    pub abitrate: u32,

    /// Resize video: WIDTHxHEIGHT — use 0 for one dimension to keep aspect ratio
    #[arg(long, value_name = "WxH")]
    pub resize: Option<String>,

    /// Soft-embed a subtitle file (.ass or .srt) as an MKV subtitle track
    #[arg(long)]
    pub sub: Option<PathBuf>,

    /// Hard burn-in subtitles into the video frames (requires --sub)
    #[arg(long)]
    pub hardsub: bool,

    /// Number of encoding threads (0 = auto)
    #[arg(long, default_value_t = 0)]
    pub threads: usize,

    /// Extra encoder options as key=value pairs, e.g. --opt preset=slow
    #[arg(long = "opt", value_name = "KEY=VALUE", num_args = 1)]
    pub opts: Vec<String>,
}
