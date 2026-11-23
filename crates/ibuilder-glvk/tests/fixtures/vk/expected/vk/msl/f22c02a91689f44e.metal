#include <metal_stdlib>
#include <simd/simd.h>

using namespace metal;

struct U
{
    float4x4 mvp;
};

struct P
{
    float4 tint;
};

struct vk_f22c02a91689f44e_out
{
    float4 col [[user(locn0)]];
    float4 gl_Position [[position]];
};

struct vk_f22c02a91689f44e_in
{
    float3 pos [[attribute(0)]];
};

vertex vk_f22c02a91689f44e_out vk_f22c02a91689f44e(vk_f22c02a91689f44e_in in [[stage_in]], constant U& u [[buffer(0)]], constant P& pc [[buffer(1)]])
{
    vk_f22c02a91689f44e_out out = {};
    out.gl_Position = u.mvp * float4(in.pos, 1.0);
    out.col = pc.tint;
    out.gl_Position.y = -(out.gl_Position.y);    // Invert Y-axis for Metal
    return out;
}

