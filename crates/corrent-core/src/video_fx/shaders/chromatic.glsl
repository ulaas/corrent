//!HOOK OUTPUT
//!BIND HOOKED
//!DESC Corrent: colour fringes
@DEFS@
vec4 hook() {
    vec4 c = HOOKED_tex(HOOKED_pos);
    // Red and blue pulled apart, more towards the corners.
    vec2 off = (HOOKED_pos - 0.5) * 2.0 * CHROMATIC_AMOUNT * PX * HOOKED_pt;
    return vec4(HOOKED_tex(HOOKED_pos + off).r, c.g, HOOKED_tex(HOOKED_pos - off).b, c.a);
}
