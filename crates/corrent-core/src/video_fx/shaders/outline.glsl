//!HOOK OUTPUT
//!BIND HOOKED
//!DESC Corrent: outlines
@DEFS@
float fx_outline_at(float x, float y) {
    return fx_luma(HOOKED_texOff(vec2(x, y) * max(PX, 1.0)).rgb);
}
vec4 hook() {
    vec4 c = HOOKED_tex(HOOKED_pos);
    float tl = fx_outline_at(-1.0, -1.0);
    float tc = fx_outline_at(0.0, -1.0);
    float tr = fx_outline_at(1.0, -1.0);
    float ml = fx_outline_at(-1.0, 0.0);
    float mr = fx_outline_at(1.0, 0.0);
    float bl = fx_outline_at(-1.0, 1.0);
    float bc = fx_outline_at(0.0, 1.0);
    float br = fx_outline_at(1.0, 1.0);
    float gx = (tr + 2.0 * mr + br) - (tl + 2.0 * ml + bl);
    float gy = (bl + 2.0 * bc + br) - (tl + 2.0 * tc + tr);
    float edge = clamp(length(vec2(gx, gy)) * 2.0, 0.0, 1.0);
    return vec4(c.rgb * (1.0 - OUTLINE_AMOUNT * edge), c.a);
}
