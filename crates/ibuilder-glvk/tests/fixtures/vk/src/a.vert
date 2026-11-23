#version 450
layout(location=0) in vec3 pos;
layout(set=1, binding=3) uniform U { mat4 mvp; } u;
layout(push_constant) uniform P { vec4 tint; } pc;
layout(location=0) out vec4 col;
void main(){ gl_Position = u.mvp*vec4(pos,1.0); col = pc.tint; }
