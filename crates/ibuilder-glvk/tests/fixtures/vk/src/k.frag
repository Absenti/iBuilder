#version 450
// the set that moves to an argument buffer holds a uniform and a storage buffer: their dynamic offsets (q: / o: rows)
layout(set = 0, binding = 0) uniform texture2D a[25];
layout(set = 0, binding = 1) uniform Params { vec4 scale; int n; } p;
layout(set = 0, binding = 4) readonly buffer Extra { vec4 add[]; } e;
layout(set = 1, binding = 0) uniform texture2D b[10];
layout(set = 1, binding = 1) uniform sampler s;
layout(location = 0) in vec2 uv;
layout(location = 0) out vec4 o;
void main() {
    vec4 acc = vec4(0.0);
    for (int i = 0; i < 25; i++) acc += texture(sampler2D(a[i], s), uv);
    for (int i = 0; i < 10; i++) acc += texture(sampler2D(b[i], s), uv);
    o = acc * p.scale + e.add[p.n];
}
