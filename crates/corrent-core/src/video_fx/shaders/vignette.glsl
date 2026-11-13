vec3 fx_vignette(vec3 c, vec2 pos) {
    float r = length(pos - 0.5) * 1.41421;
    return c * (1.0 - VIGNETTE_STRENGTH * smoothstep(VIGNETTE_RADIUS - 0.4, VIGNETTE_RADIUS + 0.3, r));
}
