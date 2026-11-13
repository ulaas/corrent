vec3 fx_cross_process(vec3 c, vec2 pos) {
    vec3 s = clamp(c, 0.0, 1.0);
    vec3 curve = s * s * (3.0 - 2.0 * s);
    vec3 o = vec3(curve.r, mix(s.g, curve.g, 0.6) * 1.05, mix(s.b, 0.25 + 0.5 * s.b, 0.7));
    return mix(c, o, CROSS_PROCESS_AMOUNT);
}
