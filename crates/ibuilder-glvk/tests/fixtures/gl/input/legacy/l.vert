#version 120
attribute vec3 pos;
attribute vec2 uv;
attribute vec4 col;
uniform mat4 mvp;
void main() {
    gl_TexCoord[0] = vec4(uv, 0.0, 1.0);
    gl_FrontColor = col;
    gl_Position = mvp * vec4(pos, 1.0);
}
