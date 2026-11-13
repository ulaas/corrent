vec3 fx_mono(vec3 c, vec2 pos) {
    return mix(c, vec3(fx_luma(c)), MONO_AMOUNT);
}
