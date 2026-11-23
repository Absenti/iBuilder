#include <metal_stdlib>
#include <simd/simd.h>

using namespace metal;

struct _RESERVED_IDENTIFIER_FIXUP_gl_DefaultUniformBlock
{
    int2 u_off;
};

struct spvDescriptorSetBuffer0
{
    texture2d<float> dm_gl_resource_20 [[id(0)]];
    sampler dm_gl_resource_20Smplr [[id(1)]];
    constant _RESERVED_IDENTIFIER_FIXUP_gl_DefaultUniformBlock* dm_gl_resource_27 [[id(2)]];
    texture2d<float> dm_gl_resource_41 [[id(3)]];
    sampler dm_gl_resource_41Smplr [[id(4)]];
};

struct gl_81bf59c0227e1b16_f_out
{
    float4 o_color [[color(0)]];
};

struct gl_81bf59c0227e1b16_f_in
{
    float4 v_col [[user(v_col)]];
    float v_w [[user(v_w)]];
};

fragment gl_81bf59c0227e1b16_f_out gl_81bf59c0227e1b16_f(gl_81bf59c0227e1b16_f_in in [[stage_in]], constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]])
{
    gl_81bf59c0227e1b16_f_out out = {};
    float2 uv = in.v_col.xy;
    float4 a = spvDescriptorSet0.dm_gl_resource_20.sample(spvDescriptorSet0.dm_gl_resource_20Smplr, (uv + (float2((*spvDescriptorSet0.dm_gl_resource_27).u_off) / float2(int2(spvDescriptorSet0.dm_gl_resource_20.get_width(), spvDescriptorSet0.dm_gl_resource_20.get_height())))));
    float4 b = spvDescriptorSet0.dm_gl_resource_41.sample(spvDescriptorSet0.dm_gl_resource_41Smplr, uv, int2(1, 0));
    out.o_color = precise::min(a, b) * in.v_w;
    return out;
}

