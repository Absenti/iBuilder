#version 120
uniform sampler2D tex;
void main() {
    vec4 c = texture2D(tex, gl_TexCoord[0].xy) * gl_Color;
    gl_FragData[0] = c;
    gl_FragData[1] = vec4(c.a);
}
