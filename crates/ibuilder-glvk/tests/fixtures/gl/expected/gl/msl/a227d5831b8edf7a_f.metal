#include <metal_stdlib>
#include <simd/simd.h>

using namespace metal;

struct spvDescriptorSetBuffer0
{
    texture2d<float> dm_gl_resource_13 [[id(0)]];
    sampler dm_gl_resource_13Smplr [[id(1)]];
};

struct gl_a227d5831b8edf7a_f_out
{
    float4 dm_LegacyFragData0 [[color(0)]];
    float4 dm_LegacyFragData1 [[color(1)]];
};

struct gl_a227d5831b8edf7a_f_in
{
    float4 dm_LegacyColor [[user(dm_LegacyColor)]];
    float4 dm_LegacyTexCoord0 [[user(dm_LegacyTexCoord0)]];
};

fragment gl_a227d5831b8edf7a_f_out gl_a227d5831b8edf7a_f(gl_a227d5831b8edf7a_f_in in [[stage_in]], constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]])
{
    gl_a227d5831b8edf7a_f_out out = {};
    float4 c = spvDescriptorSet0.dm_gl_resource_13.sample(spvDescriptorSet0.dm_gl_resource_13Smplr, in.dm_LegacyTexCoord0.xy) * in.dm_LegacyColor;
    out.dm_LegacyFragData0 = c;
    out.dm_LegacyFragData1 = float4(c.w);
    return out;
}

