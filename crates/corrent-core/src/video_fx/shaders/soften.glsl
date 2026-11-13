//!HOOK OUTPUT
//!BIND HOOKED
//!DESC Corrent: soften
@DEFS@
vec4 hook() {
    vec4 c = HOOKED_tex(HOOKED_pos);
    float d = 1.5 * PX;
    vec3 blur = c.rgb * 4.0;
    blur += 2.0 * (HOOKED_texOff(vec2(d, 0.0)).rgb + HOOKED_texOff(vec2(-d, 0.0)).rgb
        + HOOKED_texOff(vec2(0.0, d)).rgb + HOOKED_texOff(vec2(0.0, -d)).rgb);
    blur += HOOKED_texOff(vec2(d, d)).rgb + HOOKED_texOff(vec2(-d, d)).rgb
        + HOOKED_texOff(vec2(d, -d)).rgb + HOOKED_texOff(vec2(-d, -d)).rgb;
    return vec4(mix(c.rgb, blur / 16.0, SOFTEN_AMOUNT), c.a);
}
