//!HOOK OUTPUT
//!BIND HOOKED
//!DESC Corrent: pixelate
@DEFS@
vec4 hook() {
    vec2 block = max(floor(PIXELATE_BLOCK * PX), 1.0) * HOOKED_pt;
    return HOOKED_tex((floor(HOOKED_pos / block) + 0.5) * block);
}
