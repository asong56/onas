# onas

Fast image conversion, audio transcoding, and video transcoding CLI.

---

## Supported formats

| | Formats |
|---|---|
| **Image decode** | JPEG · PNG · WebP · AVIF · JXL |
| **Image encode** | JPEG · PNG · WebP · AVIF · JXL |
| **Audio decode** | FLAC · Opus · M4A/AAC · MP3 · OGG · WAV · and more |
| **Audio encode** | FLAC · Opus · M4A (AAC-LC) |
| **Video decode** | H.264 · H.265 · VP9 · AV1 (any container FFmpeg can read) |
| **Video encode** | H.264 · H.265 · VP9 · AV1 · copy (MKV container only) |
| **Metadata read** | EXIF (images) · ID3v2 · VorbisComments · MP4 ilst |
| **Metadata write** | ID3v2 · VorbisComments · MP4 ilst |

---

## Installation

Download a pre-built binary from the [Releases](../../releases) page:

| File | Platform |
|---|---|
| `onas-windows-x64.exe` | Windows 10/11 x64 |
| `onas-linux-x64` | Linux x64 |
| `onas-macos-aarch64` | macOS Apple Silicon |

**Linux:** `chmod +x onas-linux-x64`

**macOS:** On first run, allow the binary in **System Settings → Privacy & Security**.

---

## Usage

### Image

```sh
# JPEG → PNG
onas image photo.jpg photo.png

# PNG → WebP, quality 85
onas image photo.png photo.webp --quality 85

# Any → AVIF lossless
onas image photo.jpg photo.avif --lossless

# Resize to 1920×1080, then convert to JXL
onas image photo.png photo.jxl --resize 1920x1080 --quality 90

# Resize keeping aspect ratio (fix width, auto height)
onas image photo.jpg photo.jpg --resize 1280x0
```

### Audio

```sh
# FLAC → Opus at 192 kbps
onas audio track.flac track.opus --bitrate 192

# MP3 → FLAC (lossless, compression level 8)
onas audio track.mp3 track.flac --compression 8

# OGG → M4A (AAC) at 256 kbps
onas audio track.ogg track.m4a --bitrate 256

# Force output format regardless of extension
onas audio input.wav output.bin --format opus --bitrate 128
```

### Video

```sh
# MP4 H.264 → MKV H.265, Opus audio, CRF 22
onas video input.mp4 output.mkv --vcodec h265 --acodec opus --crf 22

# Any input → MKV AV1, FLAC audio
onas video input.mkv output.mkv --vcodec av1 --acodec flac --crf 28

# Copy video stream, re-encode audio only
onas video input.mkv output.mkv --vcodec copy --acodec opus --abitrate 128

# Resize to 1280×720, transcode to VP9
onas video input.mp4 output.mkv --vcodec vp9 --resize 1280x720 --crf 33

# Soft-embed external subtitle (ASS or SRT)
onas video input.mkv output.mkv --vcodec copy --acodec copy --sub subs.ass

# Hard burn-in subtitle into video frames
onas video input.mkv output.mkv --vcodec h264 --sub subs.srt --hardsub

# Extra encoder options
onas video input.mp4 output.mkv --vcodec h264 --opt preset=slow --opt tune=film

# Multi-threaded encode
onas video input.mp4 output.mkv --vcodec h265 --threads 8
```

Output container is MKV only; input can be any container FFmpeg can read.

| Codec | Decode | Encode |
|---|---|---|
| H.264 | rust_h264 (pure Rust) | x264 (vcpkg) |
| H.265 | libde265 (vcpkg) | x265 (our FFI) |
| VP9 | libvpx (vcpkg) | libvpx (vcpkg) |
| AV1 | dav1d (vcpkg) | rav1e (pure Rust) |

### Frame extraction

Pull a single still frame out of a video and save it as PNG or JPEG.

```sh
# First frame
onas frame input.mkv thumbnail.png

# Frame at a specific timestamp (seconds, or [H:]MM:SS[.mmm])
onas frame input.mkv frame.png --at 12.5
onas frame input.mkv frame.png --at 1:02.5

# Frame by zero-based decoded-frame index instead of a timestamp
onas frame input.mkv frame.jpg --at-frame 300 --quality 95

# Extract and downscale in one step
onas frame input.mkv thumb.png --at 5 --resize 640x0
```

`--at` and `--at-frame` are mutually exclusive; if neither is given, the
first decodable frame is used. Output format is taken from the output
extension (`.png`/`.jpg`/`.jpeg`) unless overridden with `--format`.

### Metadata

```sh
# Dump all metadata
onas meta photo.jpg --dump
onas meta track.flac --dump

# Set tag fields (KEY=VALUE, repeatable)
onas meta track.flac --set "TrackTitle=My Song" --set "TrackArtist=Jane Doe"
onas meta track.m4a  --set "Album=My Album" --set "Year=2025"

# Remove fields
onas meta track.mp3 --remove "COMMENT"

# Strip all metadata
onas meta photo.jpg --strip
onas meta track.flac --strip
```

---

## Scripting: exit codes, `--json`, and library use

`onas` is designed to be driven by other tools as well as interactively.

### Exit codes

Every invocation exits with one of the following codes (loosely following
the BSD `sysexits.h` convention), so a wrapping script can tell failure
modes apart without parsing stderr text:

| Code | Meaning |
|---|---|
| 0  | Success |
| 64 | Usage error — bad arguments/flags (e.g. malformed `--resize`) |
| 65 | Input data error — malformed/unreadable content, missing track, etc. |
| 66 | Input file could not be opened (missing, permissions) |
| 69 | Feature recognized but not implemented (e.g. `--hardsub`) |
| 70 | Codec error — decoder/encoder init or mid-stream failure |
| 73 | Output could not be created/written |
| 1  | Anything else (generic failure) |

### `--json`

Add `--json` to any subcommand to also print a machine-readable summary
(or, on failure, an `{"error": ..., "exit_code": ...}` object to stderr)
as the last line of output, alongside the normal human-readable line:

```sh
onas image photo.jpg photo.avif --json
# photo.jpg → photo.avif  (1920×1080)
# {"command":"image","input":"photo.jpg","output":"photo.avif","width":1920,"height":1080,"message":"1920 × 1080"}
```

### As a library

`onas` also builds as a normal Rust library crate (same source, no
subprocess needed), for programs that want to call it in-process:

```rust
use onas::{cli::ImageArgs, run_image};
use std::path::PathBuf;

let report = run_image(ImageArgs {
    input:    PathBuf::from("in.png"),
    output:   PathBuf::from("out.avif"),
    quality:  85,
    lossless: false,
    resize:   None,
})?;
println!("{}×{}", report.width.unwrap(), report.height.unwrap());
# Ok::<(), anyhow::Error>(())
```

`run_audio`, `run_video`, `run_frame`, and `run_meta` follow the same
pattern, each taking the corresponding `cli::*Args` struct and returning
a `Result<Report>`.

---

### Requirements

- Rust 1.78+
- C toolchain (MSVC on Windows, GCC/Clang on Linux/macOS)
- System libraries (see below)

### System libraries

**Windows** (via [vcpkg](https://vcpkg.io)):
```powershell
vcpkg install opus:x64-windows-static libflac:x64-windows-static fdk-aac:x64-windows-static dav1d:x64-windows-static libvpx:x64-windows-static x264:x64-windows-static libde265:x64-windows-static x265:x64-windows-static
```
`onas-windows-x64.zip` ships as a single self-contained exe (statically linked) — no extra DLLs needed at runtime.

**Linux** (Ubuntu/Debian):
```sh
sudo apt install libopus-dev libflac-dev libogg-dev libfdk-aac-dev libdav1d-dev libvpx-dev libx264-dev libde265-dev libx265-dev libnuma-dev cmake nasm
```

**macOS** (Homebrew):
```sh
brew install opus flac libogg fdk-aac dav1d libvpx x264 libde265 x265 cmake nasm
```

### Build

```sh
cargo build --release
```

Binary will be at `target/release/onas` (or `onas.exe` on Windows).

---

## Notes

- **AVIF decode** uses `dav1d` (C, via system library). AVIF encode uses `ravif` (pure Rust).
- **JXL decode** is pure Rust (`jxl-oxide`). JXL encode uses `libjxl` built from source via CMake (handled automatically by the `jpegxl-src` crate).
- **Opus output** is currently written as a length-prefixed raw packet stream. For a spec-compliant `.opus` file (Ogg container), re-mux with `opusenc` or `ffmpeg -i input.raw_opus -c copy output.opus`.
- **M4A output** is written as a raw ADTS stream. Most players accept this directly. For a proper MP4 container, mux with `mp4box` or `ffmpeg`.

---

## License

MIT
