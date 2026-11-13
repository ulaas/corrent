vec3 fx_posterize(vec3 c, vec2 pos) {
    float n = POSTERIZE_LEVELS - 1.0;
    return floor(clamp(c, 0.0, 1.0) * n + 0.5) / n;
}
