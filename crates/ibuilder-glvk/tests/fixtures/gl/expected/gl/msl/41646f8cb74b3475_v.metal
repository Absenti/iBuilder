#include <metal_stdlib>
#include <simd/simd.h>

using namespace metal;

struct _RESERVED_IDENTIFIER_FIXUP_gl_DefaultUniformBlock
{
    float4x4 u_mvp;
    float4x4 u_mvp_prev;
};

struct spvDescriptorSetBuffer0
{
    constant _RESERVED_IDENTIFIER_FIXUP_gl_DefaultUniformBlock* dm_gl_resource_22 [[id(0)]];
};

struct gl_41646f8cb74b3475_v_out
{
    float2 v_uv [[user(v_uv)]];
    float4 gl_Position [[position]];
};

struct gl_41646f8cb74b3475_v_in
{
    float4 a_pos [[attribute(0)]];
    float2 a_uv [[attribute(1)]];
};

vertex gl_41646f8cb74b3475_v_out gl_41646f8cb74b3475_v(gl_41646f8cb74b3475_v_in in [[stage_in]], constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]])
{
    gl_41646f8cb74b3475_v_out out = {};
    out.v_uv = in.a_uv;
    out.gl_Position = (*spvDescriptorSet0.dm_gl_resource_22).u_mvp * in.a_pos;
    out.gl_Position += (((*spvDescriptorSet0.dm_gl_resource_22).u_mvp * in.a_pos) * 0.0);
    out.gl_Position.z = (out.gl_Position.z + out.gl_Position.w) * 0.5;       // Adjust clip-space for Metal
    return out;
}

