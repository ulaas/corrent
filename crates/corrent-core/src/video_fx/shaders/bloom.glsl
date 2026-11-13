//!HOOK OUTPUT
//!BIND HOOKED
//!SAVE FX_BLOOM_A
//!WIDTH HOOKED.w 4 /
//!HEIGHT HOOKED.h 4 /
//!DESC Corrent: glow (the lights)
@DEFS@
vec4 hook() {
    vec3 c = 0.25 * (HOOKED_texOff(vec2(-1.0, -1.0)).rgb + HOOKED_texOff(vec2(1.0, -1.0)).rgb
        + HOOKED_texOff(vec2(-1.0, 1.0)).rgb + HOOKED_texOff(vec2(1.0, 1.0)).rgb);
    return vec4(max(c - BLOOM_THRESHOLD, 0.0) / max(1.0 - BLOOM_THRESHOLD, 0.05), 1.0);
}
//!HOOK OUTPUT
//!BIND FX_BLOOM_A
//!SAVE FX_BLOOM_B
//!WIDTH FX_BLOOM_A.w
//!HEIGHT FX_BLOOM_A.h
//!DESC Corrent: glow (across)
@DEFS@
vec4 hook() {
    float d = 1.5 * PX;
    vec3 c = 0.2270 * FX_BLOOM_A_tex(FX_BLOOM_A_pos).rgb;
    c += 0.1946 * (FX_BLOOM_A_texOff(vec2(d, 0.0)).rgb + FX_BLOOM_A_texOff(vec2(-d, 0.0)).rgb);
    c += 0.1216 * (FX_BLOOM_A_texOff(vec2(2.0 * d, 0.0)).rgb + FX_BLOOM_A_texOff(vec2(-2.0 * d, 0.0)).rgb);
    c += 0.0541 * (FX_BLOOM_A_texOff(vec2(3.0 * d, 0.0)).rgb + FX_BLOOM_A_texOff(vec2(-3.0 * d, 0.0)).rgb);
    c += 0.0162 * (FX_BLOOM_A_texOff(vec2(4.0 * d, 0.0)).rgb + FX_BLOOM_A_texOff(vec2(-4.0 * d, 0.0)).rgb);
    return vec4(c, 1.0);
}
//!HOOK OUTPUT
//!BIND FX_BLOOM_B
//!SAVE FX_BLOOM_C
//!WIDTH FX_BLOOM_B.w
//!HEIGHT FX_BLOOM_B.h
//!DESC Corrent: glow (down)
@DEFS@
vec4 hook() {
    float d = 1.5 * PX;
    vec3 c = 0.2270 * FX_BLOOM_B_tex(FX_BLOOM_B_pos).rgb;
    c += 0.1946 * (FX_BLOOM_B_texOff(vec2(0.0, d)).rgb + FX_BLOOM_B_texOff(vec2(0.0, -d)).rgb);
    c += 0.1216 * (FX_BLOOM_B_texOff(vec2(0.0, 2.0 * d)).rgb + FX_BLOOM_B_texOff(vec2(0.0, -2.0 * d)).rgb);
    c += 0.0541 * (FX_BLOOM_B_texOff(vec2(0.0, 3.0 * d)).rgb + FX_BLOOM_B_texOff(vec2(0.0, -3.0 * d)).rgb);
    c += 0.0162 * (FX_BLOOM_B_texOff(vec2(0.0, 4.0 * d)).rgb + FX_BLOOM_B_texOff(vec2(0.0, -4.0 * d)).rgb);
    return vec4(c, 1.0);
}
//!HOOK OUTPUT
//!BIND HOOKED
//!BIND FX_BLOOM_C
//!DESC Corrent: glow
@DEFS@
vec4 hook() {
    vec4 c = HOOKED_tex(HOOKED_pos);
    vec3 glow = clamp(FX_BLOOM_C_tex(FX_BLOOM_C_pos).rgb * BLOOM_INTENSITY * 1.5, 0.0, 1.0);
    // Screen: lights add up without clipping.
    return vec4(1.0 - (1.0 - clamp(c.rgb, 0.0, 1.0)) * (1.0 - glow), c.a);
}
