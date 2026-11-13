vec3 fx_vibrance(vec3 c, vec2 pos) {
    float hi = max(c.r, max(c.g, c.b));
    float lo = min(c.r, min(c.g, c.b));
    // Colours that are already strong move least.
    return mix(vec3(fx_luma(c)), c, 1.0 + VIBRANCE_AMOUNT * (1.0 - clamp(hi - lo, 0.0, 1.0)));
}
