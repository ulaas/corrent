//!HOOK OUTPUT
//!BIND HOOKED
//!DESC Corrent: sharpen
@DEFS@
vec4 hook() {
    // Contrast-adaptive: flat areas and strong edges get less than soft detail.
    vec4 c = HOOKED_tex(HOOKED_pos);
    vec3 n = HOOKED_texOff(vec2(0.0, -1.0)).rgb;
    vec3 w = HOOKED_texOff(vec2(-1.0, 0.0)).rgb;
    vec3 e = HOOKED_texOff(vec2(1.0, 0.0)).rgb;
    vec3 s = HOOKED_texOff(vec2(0.0, 1.0)).rgb;
    vec3 lo = min(c.rgb, min(min(n, w), min(e, s)));
    vec3 hi = max(c.rgb, max(max(n, w), max(e, s)));
    vec3 room = sqrt(clamp(min(lo, 1.0 - hi) / max(hi, 0.0001), 0.0, 1.0));
    vec3 weight = room * (-1.0 / mix(8.0, 5.0, SHARPEN_AMOUNT));
    vec3 o = (c.rgb + (n + w + e + s) * weight) / (1.0 + 4.0 * weight);
    return vec4(clamp(o, 0.0, 1.0), c.a);
}
