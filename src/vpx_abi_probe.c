/* Emits the libvpx decoder ABI version that vpx_codec_dec_init_ver()
 * requires. This macro is defined via preprocessor arithmetic in
 * vpx_decoder.h and is NOT captured as a constant in the pre-generated
 * bindings shipped by the `libvpx-sys` crate (rust-vpx repo, commit
 * 04df690), so we recover it at build time straight from whichever
 * libvpx headers are actually being linked against (vcpkg on Windows,
 * apt/brew elsewhere) instead of hardcoding a version-specific number.
 */
#include <vpx/vpx_decoder.h>

int onas_vpx_decoder_abi_version(void) {
    return VPX_DECODER_ABI_VERSION;
}
