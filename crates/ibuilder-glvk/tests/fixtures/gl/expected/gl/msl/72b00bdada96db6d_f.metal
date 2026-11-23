#include <metal_stdlib>
#include <simd/simd.h>

using namespace metal;

struct _RESERVED_IDENTIFIER_FIXUP_gl_DefaultUniformBlock
{
    uint flip;
    float4 tint;
};

struct spvDescriptorSetBuffer0
{
    constant _RESERVED_IDENTIFIER_FIXUP_gl_DefaultUniformBlock* dm_gl_resource_14 [[id(0)]];
    texture2d<float> dm_gl_resource_38 [[id(1)]];
    sampler dm_gl_resource_38Smplr [[id(2)]];
};

struct gl_72b00bdada96db6d_f_out
{
    float4 gm_FragColor [[color(0)]];
};

struct gl_72b00bdada96db6d_f_in
{
    float2 v_uv [[user(v_uv)]];
};

fragment gl_72b00bdada96db6d_f_out gl_72b00bdada96db6d_f(gl_72b00bdada96db6d_f_in in [[stage_in]], constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]])
{
    gl_72b00bdada96db6d_f_out out = {};
    float2 _23;
    if ((*spvDescriptorSet0.dm_gl_resource_14).flip != 0u)
    {
        _23 = in.v_uv.yx;
    }
    else
    {
        _23 = in.v_uv;
    }
    float2 uv = _23;
    out.gm_FragColor = spvDescriptorSet0.dm_gl_resource_38.sample(spvDescriptorSet0.dm_gl_resource_38Smplr, uv) * (*spvDescriptorSet0.dm_gl_resource_14).tint;
    return out;
}

