vec3 fx_tone(vec3 c, vec2 pos) {
    float l = clamp(fx_luma(c), 0.0, 1.0);
    float lift = TONE_SHADOWS * 0.35 * pow(1.0 - l, 3.0) + TONE_HIGHLIGHTS * 0.35 * pow(l, 3.0);
    return c * (1.0 + lift / max(l, 0.25));
}
