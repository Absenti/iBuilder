#version 310 es
precision highp float;
layout(binding = 2) uniform sampler2D u_tex;
uniform sampler2D u_t2;
uniform sampler2D u_never;
uniform ivec2 u_off;
in vec4 v_col;
in float v_w;
layout(location = 0) out vec4 o_color;
void main() {
    vec2 uv = v_col.xy;
    vec4 a = textureOffset(u_tex, uv, ivec2(u_off));
    vec4 b = textureOffset(u_t2, uv, ivec2(1, 0));
    o_color = min(a, b) * v_w;
}
