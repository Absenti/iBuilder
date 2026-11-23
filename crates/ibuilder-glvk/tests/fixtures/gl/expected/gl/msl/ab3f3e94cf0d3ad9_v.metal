#include <metal_stdlib>
#include <simd/simd.h>

using namespace metal;

struct _RESERVED_IDENTIFIER_FIXUP_gl_DefaultUniformBlock
{
    float4x4 u_projTrans;
};

struct spvDescriptorSetBuffer0
{
    constant _RESERVED_IDENTIFIER_FIXUP_gl_DefaultUniformBlock* dm_gl_resource_21 [[id(0)]];
};

struct gl_ab3f3e94cf0d3ad9_v_out
{
    float4 v_color [[user(v_color)]];
    float4 gl_Position [[position]];
};

struct gl_ab3f3e94cf0d3ad9_v_in
{
    float4 a_position [[attribute(0)]];
    float4 a_color [[attribute(1)]];
};

vertex gl_ab3f3e94cf0d3ad9_v_out gl_ab3f3e94cf0d3ad9_v(gl_ab3f3e94cf0d3ad9_v_in in [[stage_in]], constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]])
{
    gl_ab3f3e94cf0d3ad9_v_out out = {};
    out.v_color = in.a_color;
    out.gl_Position = (*spvDescriptorSet0.dm_gl_resource_21).u_projTrans * in.a_position;
    out.gl_Position.z = (out.gl_Position.z + out.gl_Position.w) * 0.5;       // Adjust clip-space for Metal
    return out;
}

