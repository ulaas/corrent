//!HOOK MAIN
//!BIND HOOKED
//!DESC Corrent: denoise
@DEFS@
vec4 hook() {
    vec4 c = HOOKED_tex(HOOKED_pos);
    vec3 sum = c.rgb;
    float total = 1.0;
    // Neighbours count by how near they are, in place and in colour: edges stay.
    for (int y = -2; y <= 2; y++) {
        for (int x = -2; x <= 2; x++) {
            if (x == 0 && y == 0) continue;
            vec3 s = HOOKED_texOff(vec2(float(x), float(y))).rgb;
            vec3 d = s - c.rgb;
            float w = exp(-dot(d, d) / (2.0 * DENOISE_EDGES * DENOISE_EDGES) - float(x * x + y * y) / 8.0);
            sum += s * w;
            total += w;
        }
    }
    return vec4(mix(c.rgb, sum / total, DENOISE_STRENGTH), c.a);
}
