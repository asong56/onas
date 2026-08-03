/* Emits X265_BUILD, the libx265 ABI/soname number baked into the
 * versioned `x265_encoder_open_<BUILD>` symbol name (see x265.h's
 * x265_encoder_glue1/2 macros). It's `#define`d in x265_config.h and
 * changes between libx265 releases, so we recover it at build time from
 * whichever headers are actually being linked against (vcpkg on Windows,
 * pkg-config elsewhere) rather than hardcoding a number that would go
 * stale. Used by src/lib.rs to call the stable, version-skew-tolerant
 * x265_api_query() entry point instead of linking the versioned symbol
 * directly.
 */
#include <x265_config.h>

int onas_x265_build_number(void) {
    return X265_BUILD;
}
