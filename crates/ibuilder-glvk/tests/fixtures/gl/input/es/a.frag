#version 100
precision mediump float;
uniform sampler2D texture;
uniform bool flip;
uniform lowp vec4 tint;
varying vec2 v_uv;
void main() {
    vec2 uv = flip ? v_uv.yx : v_uv;
    gl_FragColor = texture2D(texture, uv) * tint;
}
