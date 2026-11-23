#include <metal_stdlib>
#include <simd/simd.h>

using namespace metal;

struct Params
{
    float4 scale;
    int n;
};

struct Extra
{
    float4 add[1];
};

struct spvDescriptorSetBuffer0
{
    array<texture2d<float>, 20> a [[id(0)]];
    sampler s [[id(20)]];
};

struct vk_014251df2ae73ef9_out
{
    float4 o [[color(0)]];
};

struct vk_014251df2ae73ef9_in
{
    float2 uv [[user(locn0)]];
};

fragment vk_014251df2ae73ef9_out vk_014251df2ae73ef9(vk_014251df2ae73ef9_in in [[stage_in]], constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(2)]], constant Params& p [[buffer(0)]], const device Extra& e [[buffer(1)]], array<texture2d<float>, 20> b [[texture(0)]])
{
    vk_014251df2ae73ef9_out out = {};
    float4 acc = float4(0.0);
    for (int i = 0; i < 20; i++)
    {
        acc += (spvDescriptorSet0.a[i].sample(spvDescriptorSet0.s, in.uv) + b[i].sample(spvDescriptorSet0.s, in.uv));
    }
    out.o = (acc * p.scale) + e.add[p.n];
    return out;
}

