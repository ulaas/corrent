vec3 fx_grain(vec3 c, vec2 pos) {
    vec2 cell = floor(pos * HOOKED_size / (GRAIN_SIZE * PX));
    float n = fx_hash(cell + vec2(random * 613.0, random * 157.0)) - 0.5;
    // Less of it in the lights, as on film.
    return c + n * GRAIN_AMOUNT * (1.0 - 0.6 * clamp(fx_luma(c), 0.0, 1.0));
}
