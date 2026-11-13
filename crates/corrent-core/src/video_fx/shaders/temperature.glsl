vec3 fx_temperature(vec3 c, vec2 pos) {
    vec3 gain = vec3(
        1.0 + 0.22 * TEMPERATURE_WARMTH + 0.08 * TEMPERATURE_TINT,
        1.0 - 0.16 * TEMPERATURE_TINT,
        1.0 - 0.22 * TEMPERATURE_WARMTH + 0.08 * TEMPERATURE_TINT);
    vec3 o = c * gain;
    // The same brightness as before.
    return o * (fx_luma(c) / max(fx_luma(o), 0.0001));
}
