float fx_halftone_m2(vec2 p) {
    // The 2x2 Bayer matrix 0 2 / 3 1.
    return 2.0 * p.x + 3.0 * p.y - 4.0 * p.x * p.y;
}
vec3 fx_halftone(vec3 c, vec2 pos) {
    vec2 cell = floor(pos * HOOKED_size / (HALFTONE_CELL * PX));
    // 4x4 Bayer from two levels of 2x2.
    float t = (4.0 * fx_halftone_m2(mod(cell, 2.0)) + fx_halftone_m2(mod(floor(cell / 2.0), 2.0)) + 0.5) / 16.0;
    float n = HALFTONE_LEVELS - 1.0;
    return floor(clamp(c, 0.0, 1.0) * n + t) / n;
}
