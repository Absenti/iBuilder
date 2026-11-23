#version 450
// RenderDragon's material vertex shader declares its bone index input as a signed int (vk_shaders.py unsigned_bone_indices)
layout(location = 0) in vec3 pos;
layout(location = 3) in int a_indices;
layout(set = 0, binding = 0) uniform Bones { mat4 m[16]; } bones;
void main() {
    gl_Position = bones.m[a_indices] * vec4(pos, 1.0) + vec4(float(a_indices));
}
