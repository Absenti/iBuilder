#include <metal_stdlib>
#include <simd/simd.h>

using namespace metal;

struct vk_ff7028c9e00c68c7_vp1_out
{
    uint m_5 [[viewport_array_index]];
    float4 gl_Position [[position]];
};

struct vk_ff7028c9e00c68c7_vp1_in
{
    float4 m_2 [[attribute(0)]];
};

vertex vk_ff7028c9e00c68c7_vp1_out vk_ff7028c9e00c68c7_vp1(vk_ff7028c9e00c68c7_vp1_in in [[stage_in]], uint gl_InstanceIndex [[instance_id]])
{
    vk_ff7028c9e00c68c7_vp1_out out = {};
    out.gl_Position = in.m_2;
    out.m_5 = gl_InstanceIndex;
    out.gl_Position.y = -(out.gl_Position.y);    // Invert Y-axis for Metal
    return out;
}

