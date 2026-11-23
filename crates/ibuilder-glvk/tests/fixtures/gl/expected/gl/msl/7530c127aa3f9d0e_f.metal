#include <metal_stdlib>
#include <simd/simd.h>

using namespace metal;

struct _RESERVED_IDENTIFIER_FIXUP_gl_DefaultUniformBlock
{
    float4 u_tint;
};

struct spvDescriptorSetBuffer0
{
    texture2d<float> dm_gl_resource_13 [[id(0)]];
    sampler dm_gl_resource_13Smplr [[id(1)]];
    constant _RESERVED_IDENTIFIER_FIXUP_gl_DefaultUniformBlock* dm_gl_resource_22 [[id(2)]];
};

struct gl_7530c127aa3f9d0e_f_out
{
    float4 o_color [[color(0)]];
};

struct gl_7530c127aa3f9d0e_f_in
{
    float2 v_uv [[user(v_uv)]];
};

fragment gl_7530c127aa3f9d0e_f_out gl_7530c127aa3f9d0e_f(gl_7530c127aa3f9d0e_f_in in [[stage_in]], constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]])
{
    gl_7530c127aa3f9d0e_f_out out = {};
    out.o_color = spvDescriptorSet0.dm_gl_resource_13.sample(spvDescriptorSet0.dm_gl_resource_13Smplr, in.v_uv) * (*spvDescriptorSet0.dm_gl_resource_22).u_tint;
    return out;
}

