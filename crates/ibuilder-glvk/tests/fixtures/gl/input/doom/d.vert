#version 150
#define PC
uniform matrices_ubo { vec4 matrices[8]; };
in vec4 attr_Vertex;
in vec4 attr_Color;
out vec4 gl_Position;
out vec4 gl_FrontColor;
out vec2 vofi_TexCoord0;
void main() {
    gl_Position.x = dot(attr_Vertex, matrices[0]);
    gl_Position.y = dot(attr_Vertex, matrices[1]);
    gl_Position.z = dot(attr_Vertex, matrices[2]);
    gl_Position.w = dot(attr_Vertex, matrices[3]);
    gl_FrontColor = attr_Color;
    vofi_TexCoord0 = attr_Vertex.xy;
}
