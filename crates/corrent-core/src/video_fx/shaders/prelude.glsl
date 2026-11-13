#define PX max(target_size.y / 1080.0, 0.5)
float fx_luma(vec3 c) { return dot(c, vec3(0.2126, 0.7152, 0.0722)); }
float fx_hash(vec2 p) { return fract(sin(dot(p, vec2(127.1, 311.7))) * 43758.5453); }
