vec3 fx_zebra(vec3 c, vec2 pos) {
    vec2 p = pos * HOOKED_size / PX;
    float stripe = step(0.5, fract((p.x + p.y) / 12.0 + float(frame) * 0.04));
    float clipped = step(ZEBRA_THRESHOLD, max(c.r, max(c.g, c.b)));
    return mix(c, vec3(stripe), 0.85 * clipped);
}
