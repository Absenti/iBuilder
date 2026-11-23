attribute vec4 a_position;
attribute vec2 a_texcoord;
uniform mat4 u_matrix;
varying mediump vec2 v_uv;
void main() {
    vec2 sample = a_texcoord;
    v_uv = sample;
    gl_Position = u_matrix * a_position;
}
