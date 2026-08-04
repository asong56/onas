//! `onas` as a library.
//!
//! `onas` is primarily a CLI tool, but its subcommands are also exposed
//! here as a normal Rust API so other programs can call into it directly
//! (in-process) instead of spawning `onas` as a subprocess and scraping
//! stdout/exit codes. This is the same underlying code the CLI binary
//! (`src/main.rs`) uses — the binary is a thin wrapper over this crate.
//!
//! # Example
//! ```no_run
//! use onas::{cli::ImageArgs, Report};
//! use std::path::PathBuf;
//!
//! let args = ImageArgs {
//!     input:    PathBuf::from("in.png"),
//!     output:   PathBuf::from("out.avif"),
//!     quality:  85,
//!     lossless: false,
//!     resize:   None,
//! };
//! let report: Report = onas::run_image(args)?;
//! println!("{}×{}", report.width.unwrap(), report.height.unwrap());
//! # Ok::<(), anyhow::Error>(())
//! ```
//!
//! For subprocess callers that can't link the crate directly (other
//! languages, sandboxed tools, …), see [`exitcode`] for the process exit
//! code contract, and the CLI's `--json` flag for a machine-readable
//! version of the same [`Report`] on stdout.

pub mod cli;
pub mod exitcode;
pub mod image;
pub mod audio;
pub mod video;
pub mod meta;

use anyhow::Result;
use std::path::PathBuf;

/// Structured result of a single `onas` operation, meant for both
/// programmatic library callers and `--json` CLI output. Fields are
/// `Option` because not every subcommand produces every kind of value
/// (e.g. `meta --dump` has no output path; `image`/`video`/`frame` always
/// have `width`/`height`; `audio` never does).
#[derive(Debug, Clone, serde::Serialize)]
pub struct Report {
    /// Which subcommand produced this report.
    pub command: &'static str,
    /// Input file path, if the operation took one.
    pub input: Option<PathBuf>,
    /// Output file path, if the operation wrote one.
    pub output: Option<PathBuf>,
    /// Output pixel width, for image/video/frame operations.
    pub width: Option<u32>,
    /// Output pixel height, for image/video/frame operations.
    pub height: Option<u32>,
    /// Free-form human-readable summary — the same text the CLI prints
    /// to stdout on success (e.g. "in.mkv → out.mkv (1280×720 H265 CRF 23)").
    pub message: String,
}

/// Convert image files between formats. See [`cli::ImageArgs`].
pub fn run_image(args: cli::ImageArgs) -> Result<Report> {
    let input  = args.input.clone();
    let output = args.output.clone();
    let (w, h) = image::run_capture(args)?;
    Ok(Report {
        command: "image",
        input: Some(input),
        output: Some(output.clone()),
        width: Some(w),
        height: Some(h),
        message: format!("{} × {}", w, h),
    })
}

/// Transcode audio files between codecs. See [`cli::AudioArgs`].
pub fn run_audio(args: cli::AudioArgs) -> Result<Report> {
    let input  = args.input.clone();
    let output = args.output.clone();
    audio::run(args)?;
    Ok(Report {
        command: "audio",
        input: Some(input),
        output: Some(output),
        width: None,
        height: None,
        message: "ok".to_owned(),
    })
}

/// Transcode video files between codecs. See [`cli::VideoArgs`].
pub fn run_video(args: cli::VideoArgs) -> Result<Report> {
    let input  = args.input.clone();
    let output = args.output.clone();
    video::run(args)?;
    Ok(Report {
        command: "video",
        input: Some(input),
        output: Some(output),
        width: None,
        height: None,
        message: "ok".to_owned(),
    })
}

/// Extract a single still frame from a video. See [`cli::FrameArgs`].
pub fn run_frame(args: cli::FrameArgs) -> Result<Report> {
    let input  = args.input.clone();
    let output = args.output.clone();
    let (w, h) = video::run_frame_capture(args)?;
    Ok(Report {
        command: "frame",
        input: Some(input),
        output: Some(output),
        width: Some(w),
        height: Some(h),
        message: format!("{} × {}", w, h),
    })
}

/// Read or edit file metadata. See [`cli::MetaArgs`].
pub fn run_meta(args: cli::MetaArgs) -> Result<Report> {
    let input = args.file.clone();
    meta::run(args)?;
    Ok(Report {
        command: "meta",
        input: Some(input),
        output: None,
        width: None,
        height: None,
        message: "ok".to_owned(),
    })
}
