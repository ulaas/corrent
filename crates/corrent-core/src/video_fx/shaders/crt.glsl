//!HOOK OUTPUT
//!BIND HOOKED
//!DESC Corrent: tube television
@DEFS@
vec4 hook() {
    // The bulge of the glass: the middles of the edges stay where they are, the corners go dark.
    vec2 uv = HOOKED_pos * 2.0 - 1.0;
    uv *= (1.0 + CRT_CURVATURE * dot(uv, uv)) / (1.0 + CRT_CURVATURE);
    vec2 p = uv * 0.5 + 0.5;
    vec4 c = HOOKED_tex(p);
    vec2 inside = step(vec2(0.0), p) * step(p, vec2(1.0));
    vec3 o = c.rgb * inside.x * inside.y;
    // Whole pixels, or the columns of the mask come out in uneven widths.
    float unit = max(floor(PX + 0.5), 1.0);
    float line = p.y * HOOKED_size.y / (3.0 * unit);
    o *= 1.0 - CRT_SCANLINES * 0.7 * (0.5 - 0.5 * cos(6.2831853 * line));
    float column = mod(floor(HOOKED_pos.x * HOOKED_size.x / unit), 3.0);
    vec3 phosphor = vec3(step(column, 0.5), step(abs(column - 1.0), 0.5), step(1.5, column));
    o *= mix(vec3(1.0), 0.6 + 0.8 * phosphor, CRT_MASK);
    o *= 1.0 + 0.35 * CRT_SCANLINES + 0.1 * CRT_MASK;
    return vec4(clamp(o, 0.0, 1.0), c.a);
}
