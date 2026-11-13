vec3 fx_night_vision(vec3 c, vec2 pos) {
    vec2 cell = floor(pos * HOOKED_size / PX);
    float l = fx_luma(c) * NIGHT_VISION_GAIN;
    l += (fx_hash(cell + vec2(random * 917.0, random * 331.0)) - 0.5) * NIGHT_VISION_NOISE;
    vec2 d = pos - 0.5;
    l *= 1.0 - 1.2 * dot(d, d);
    return vec3(0.12, 1.0, 0.22) * clamp(l, 0.0, 1.0);
}
