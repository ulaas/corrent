vec3 fx_levels(vec3 c, vec2 pos) {
    c = clamp((c - LEVELS_BLACK) / max(LEVELS_WHITE - LEVELS_BLACK, 0.01), 0.0, 1.0);
    return pow(c, vec3(1.0 / LEVELS_MIDTONES));
}
