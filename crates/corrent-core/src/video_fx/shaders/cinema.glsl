vec3 fx_cinema(vec3 c, vec2 pos) {
    float l = clamp(fx_luma(c), 0.0, 1.0);
    // Teal in the shadows, orange in the lights, and a little more contrast.
    vec3 o = c * mix(vec3(0.84, 1.0, 1.12), vec3(1.12, 1.0, 0.84), smoothstep(0.0, 1.0, l));
    o = clamp(o, 0.0, 1.0);
    o = mix(o, o * o * (3.0 - 2.0 * o), 0.5);
    return mix(c, o, CINEMA_AMOUNT);
}
