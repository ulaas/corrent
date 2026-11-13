//!HOOK OUTPUT
//!BIND HOOKED
//!DESC Corrent: clarity
@DEFS@
vec4 hook() {
    vec4 c = HOOKED_tex(HOOKED_pos);
    float r = CLARITY_RADIUS * PX;
    vec3 blur = c.rgb;
    // Two rings of six.
    for (int i = 0; i < 6; i++) {
        float a = float(i) * 1.0471976;
        blur += HOOKED_texOff(vec2(cos(a), sin(a)) * r).rgb;
        blur += HOOKED_texOff(vec2(cos(a + 0.5235988), sin(a + 0.5235988)) * r * 0.5).rgb;
    }
    blur /= 13.0;
    float l = clamp(fx_luma(c.rgb), 0.0, 1.0);
    // Local contrast, mostly in the midtones.
    float detail = (l - fx_luma(blur)) * (1.0 - abs(2.0 * l - 1.0));
    return vec4(clamp(c.rgb + 1.5 * CLARITY_AMOUNT * detail, 0.0, 1.0), c.a);
}
