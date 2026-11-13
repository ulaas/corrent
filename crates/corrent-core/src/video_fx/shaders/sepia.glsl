vec3 fx_sepia(vec3 c, vec2 pos) {
    vec3 o = vec3(
        dot(c, vec3(0.393, 0.769, 0.189)),
        dot(c, vec3(0.349, 0.686, 0.168)),
        dot(c, vec3(0.272, 0.534, 0.131)));
    return mix(c, o, SEPIA_AMOUNT);
}
