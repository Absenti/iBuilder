#version 150
uniform vec4 _fa_[2];
uniform sampler2D samp0;
in vec4 gl_FragCoord;
in vec4 gl_Color;
in vec2 vofi_TexCoord0;
out vec4 gl_FragColor;
vec4 tex2D(sampler2D sampler, vec2 texcoord) { return texture(sampler, texcoord.xy); }
void main() {
    vec4 c = tex2D(samp0, vofi_TexCoord0) * gl_Color;
    gl_FragColor = c * _fa_[0] + _fa_[1] * gl_FragCoord.x;
}
