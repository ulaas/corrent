vec3 fx_colour_blind_see(vec3 c, float kind) {
    vec3 l = pow(clamp(c, 0.0, 1.0), vec3(2.2));
    vec3 o;
    if (kind < 0.5) {
        o = vec3(
            dot(l, vec3(0.152286, 1.052583, -0.204868)),
            dot(l, vec3(0.114503, 0.786281, 0.099216)),
            dot(l, vec3(-0.003882, -0.048116, 1.051998)));
    } else if (kind < 1.5) {
        o = vec3(
            dot(l, vec3(0.367322, 0.860646, -0.227968)),
            dot(l, vec3(0.280085, 0.672501, 0.047413)),
            dot(l, vec3(-0.011820, 0.042940, 0.968881)));
    } else {
        o = vec3(
            dot(l, vec3(1.255528, -0.076749, -0.178779)),
            dot(l, vec3(-0.078411, 0.930809, 0.147602)),
            dot(l, vec3(0.004733, 0.691367, 0.303900)));
    }
    return pow(clamp(o, 0.0, 1.0), vec3(1.0 / 2.2));
}
vec3 fx_colour_blind(vec3 c, vec2 pos) {
    return mix(c, fx_colour_blind_see(c, COLOUR_BLIND_KIND), COLOUR_BLIND_SEVERITY);
}
