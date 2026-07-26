# onas

Fast image conversion and audio transcoding CLI.

---

## Supported formats

| | Formats |
|---|---|
| **Image decode** | JPEG · PNG · WebP · AVIF · JXL |
| **Image encode** | JPEG · PNG · WebP · AVIF · JXL |
| **Audio decode** | FLAC · Opus · M4A/AAC · MP3 · OGG · WAV · and more |
| **Audio encode** | FLAC · Opus · M4A (AAC-LC) |
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

## Build from source

### Requirements

- Rust 1.78+
- C toolchain (MSVC on Windows, GCC/Clang on Linux/macOS)
- System libraries (see below)

### System libraries

**Windows** (via [vcpkg](https://vcpkg.io)):
```powershell
vcpkg install libopus:x64-windows-static libflac:x64-windows-static fdk-aac:x64-windows-static dav1d:x64-windows-static
```

**Linux** (Ubuntu/Debian):
```sh
sudo apt install libopus-dev libflac-dev libfdk-aac-dev libdav1d-dev cmake nasm
```

**macOS** (Homebrew):
```sh
brew install opus flac fdk-aac dav1d cmake nasm
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

---

## Video

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

Supported video codecs: `h264`, `h265`, `vp9`, `av1`, `copy`
Supported audio codecs: `opus`, `aac`, `flac`, `copy`
Output container: MKV only. Input: any container FFmpeg can read.

### Video setup notes

**Windows**: `onas-windows-x64.zip` is a single self-contained exe (statically linked). No extra DLLs needed.

**Linux**: `sudo apt install libx264-dev libde265-dev libx265-dev libvpx-dev libdav1d-dev`

**macOS**: `brew install x264 libde265 x265 libvpx dav1d`

### Codec details

| Codec | Decode | Encode |
|---|---|---|
| H.264 | rust_h264 (pure Rust) | x264 (vcpkg) |
| H.265 | libde265 (vcpkg) | x265 (our FFI) |
| VP9 | libvpx (vcpkg) | libvpx (vcpkg) |
| AV1 | dav1d (vcpkg) | rav1e (pure Rust) |
