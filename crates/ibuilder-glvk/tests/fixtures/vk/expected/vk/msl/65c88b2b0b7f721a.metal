#include <metal_stdlib>
#include <simd/simd.h>

using namespace metal;

struct Bones
{
    float4x4 m[16];
};

struct vk_65c88b2b0b7f721a_out
{
    float4 gl_Position [[position]];
};

struct vk_65c88b2b0b7f721a_in
{
    float3 pos [[attribute(0)]];
    uint a_indices [[attribute(3)]];
};

vertex vk_65c88b2b0b7f721a_out vk_65c88b2b0b7f721a(vk_65c88b2b0b7f721a_in in [[stage_in]], constant Bones& bones [[buffer(0)]])
{
    vk_65c88b2b0b7f721a_out out = {};
    out.gl_Position = (bones.m[int(in.a_indices)] * float4(in.pos, 1.0)) + float4(float(int(in.a_indices)));
    out.gl_Position.y = -(out.gl_Position.y);    // Invert Y-axis for Metal
    return out;
}

