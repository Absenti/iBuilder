#version 310 es
precision highp float;
struct DirLights { vec3 dir[2]; vec4 color; };
struct LightEnv { DirLights m_dirLights; float m_ambient; };
uniform LightEnv u_LightEnv;
uniform float u_weights[4];
uniform bool u_on;
uniform float u_unused;
in vec3 a_pos;
in vec3 a_nrm;
out vec4 v_col;
out float v_w;
void main() {
    float l = max(dot(a_nrm, u_LightEnv.m_dirLights.dir[0]), 0.0) + u_LightEnv.m_ambient;
    v_col = u_on ? u_LightEnv.m_dirLights.color * l : vec4(l);
    v_w = u_weights[0] + u_weights[3];
    gl_Position = vec4(a_pos, 1.0);
}
