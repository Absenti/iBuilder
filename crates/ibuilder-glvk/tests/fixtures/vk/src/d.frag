#version 450
// more textures than Metal's 31 slots over two sets: one set moves to an argument buffer, with its uniform and storage buffers (dynamic offsets)
layout(set = 0, binding = 0) uniform texture2D a[20];
layout(set = 0, binding = 1) uniform sampler s;
layout(set = 1, binding = 0) uniform texture2D b[20];
layout(set = 1, binding = 2) uniform Params { vec4 scale; int n; } p;
layout(set = 1, binding = 3) readonly buffer Extra { vec4 add[]; } e;
layout(location = 0) in vec2 uv;
layout(location = 0) out vec4 o;
void main() {
    vec4 acc = vec4(0.0);
    for (int i = 0; i < 20; i++) {
        acc += texture(sampler2D(a[i], s), uv) + texture(sampler2D(b[i], s), uv);
    }
    o = acc * p.scale + e.add[p.n];
}
