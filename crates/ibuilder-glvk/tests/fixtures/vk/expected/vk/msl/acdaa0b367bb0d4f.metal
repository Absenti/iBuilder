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
    array<texture2d<float>, 25> a [[id(0)]];
    constant Params* p [[id(25)]];
    const device Extra* e [[id(26)]];
};

struct vk_acdaa0b367bb0d4f_out
{
    float4 o [[color(0)]];
};

struct vk_acdaa0b367bb0d4f_in
{
    float2 uv [[user(locn0)]];
};

fragment vk_acdaa0b367bb0d4f_out vk_acdaa0b367bb0d4f(vk_acdaa0b367bb0d4f_in in [[stage_in]], constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]], constant uint* spvDynamicOffsets [[buffer(1)]], array<texture2d<float>, 10> b [[texture(0)]], sampler s [[sampler(0)]])
{
    constant auto& p = *(constant Params* )((constant char* )spvDescriptorSet0.p + spvDynamicOffsets[0]);
    const device auto& e = *(const device Extra* )((const device char* )spvDescriptorSet0.e + spvDynamicOffsets[1]);
    vk_acdaa0b367bb0d4f_out out = {};
    float4 acc = float4(0.0);
    for (int i = 0; i < 25; i++)
    {
        acc += spvDescriptorSet0.a[i].sample(s, in.uv);
    }
    for (int i_1 = 0; i_1 < 10; i_1++)
    {
        acc += b[i_1].sample(s, in.uv);
    }
    out.o = (acc * p.scale) + e.add[p.n];
    return out;
}

