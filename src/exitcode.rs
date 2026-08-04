//! Standard process exit codes.
//!
//! `onas` is designed to be driven both interactively and as a subprocess
//! by other tools (batch converters, CI pipelines, media servers, …).
//! Callers that only see the exit status — not stderr — need to be able
//! to tell "bad input" apart from "bad arguments" apart from "codec
//! failed" without scraping log text. These codes are a stable contract:
//! once released, a given code's meaning does not change across onas
//! versions (new codes may be added).
//!
//! Loosely follows the BSD `sysexits.h` convention (used by sendmail,
//! postfix, and many other Unix tools) rather than inventing something
//! bespoke, since that convention is already widely recognized by
//! process-supervision and scripting tooling.

/// Success.
pub const OK: i32 = 0;

/// Catch-all for errors that don't fit a more specific code below.
/// Mirrors the conventional Unix "generic failure" exit status.
pub const GENERAL_ERROR: i32 = 1;

/// Command-line usage error: missing/invalid arguments, bad flag
/// combinations, unparseable values (e.g. `--resize` not WIDTHxHEIGHT).
/// Mirrors `sysexits.h`'s `EX_USAGE`.
pub const USAGE: i32 = 64;

/// Input data was the wrong kind, malformed, or otherwise unusable —
/// e.g. an image whose bytes don't match its extension, a corrupt/
/// truncated MKV, or a container missing a required track.
/// Mirrors `sysexits.h`'s `EX_DATAERR`.
pub const DATA_ERROR: i32 = 65;

/// The input file could not be opened/read at all (missing, unreadable,
/// wrong permissions) — distinct from `DATA_ERROR`, which assumes the
/// file was readable but its *contents* were the problem.
/// Mirrors `sysexits.h`'s `EX_NOINPUT`.
pub const NO_INPUT: i32 = 66;

/// The output path could not be created/written (missing parent
/// directory, permissions, disk full, …).
/// Mirrors `sysexits.h`'s `EX_CANTCREAT`.
pub const CANT_CREATE_OUTPUT: i32 = 73;

/// A required codec/decoder/encoder could not be initialized, or failed
/// mid-stream in a way that isn't attributable to bad input data (e.g.
/// the underlying C library rejected an option combination).
/// Mirrors `sysexits.h`'s `EX_SOFTWARE` (internal software error).
pub const CODEC_ERROR: i32 = 70;

/// Feature recognized but intentionally unimplemented (e.g. `--hardsub`).
/// Mirrors `sysexits.h`'s `EX_UNAVAILABLE`.
pub const UNIMPLEMENTED: i32 = 69;

/// Classifies an [`anyhow::Error`] into one of the exit codes above by
/// inspecting the error chain. This is necessarily heuristic — errors
/// are attached with `.context(...)` at the call site rather than typed
/// variants, so we look for recognizable phrases in the outermost
/// contexts. Anything unrecognized falls back to [`GENERAL_ERROR`] so a
/// classification miss is never worse than today's blanket exit(1).
pub fn classify(err: &anyhow::Error) -> i32 {
    for cause in err.chain() {
        let msg = cause.to_string();
        let lower = msg.to_ascii_lowercase();

        if lower.contains("not yet implemented") {
            return UNIMPLEMENTED;
        }
        if lower.starts_with("reading ") || lower.starts_with("opening ") {
            // our `.with_context(|| format!("reading {}", path.display()))`
            // / `"opening {}"` convention, used for input-file access
            return NO_INPUT;
        }
        if lower.starts_with("writing ") || lower.starts_with("creating ") {
            return CANT_CREATE_OUTPUT;
        }
        // Checked before the broad codec-keyword bucket below, since these
        // phrases happen to contain "codec"/"track" but describe a bad
        // *input container* (missing/mismatched track), not a codec
        // library failure.
        if lower.contains("no video track") || lower.contains("no audio track")
            || lower.contains("no codec params") || lower.contains("not audio codec params")
            || lower.contains("video track missing") {
            return DATA_ERROR;
        }
        if lower.contains("decode") || lower.contains("encode")
            || lower.contains("encoder") || lower.contains("decoder")
            || lower.contains("codec") {
            return CODEC_ERROR;
        }
        if lower.contains("must be")
            || lower.contains("cannot detect")
            || lower.contains("unknown")
            || lower.contains("invalid")
            || lower.contains("requires --")
        {
            return USAGE;
        }
        if lower.contains("parse") || lower.contains("malformed")
            || lower.contains("bad pixel buffer") || lower.contains("bad buffer")
            || lower.contains("missing") {
            return DATA_ERROR;
        }
    }
    GENERAL_ERROR
}
