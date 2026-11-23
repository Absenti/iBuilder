#version 450
// separate image + sampler, a combined sampler, an unused buffer, two outputs of the same type (the dual-source variant), max() (precise::)
layout(set = 0, binding = 0) uniform texture2D t;
layout(set = 0, binding = 1) uniform sampler s;
layout(set = 0, binding = 2) uniform sampler2D cs;
layout(set = 2, binding = 7) uniform Unused { vec4 never; } unused;
layout(location = 0) in vec2 uv;
layout(location = 0) out vec4 o0;
layout(location = 1) out vec4 o1;
void main() {
    o0 = max(texture(sampler2D(t, s), uv), vec4(0.25));
    o1 = texture(cs, uv.yx);
}
