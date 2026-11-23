#version 450
// a geometry stage glslang writes with gl_PerVertex blocks: not the pass-through shape the proof accepts, skipped and counted
layout(triangles) in;
layout(triangle_strip, max_vertices = 3) out;
void main() {
    for (int i = 0; i < 3; i++) {
        gl_Position = gl_in[i].gl_Position;
        EmitVertex();
    }
    EndPrimitive();
}
