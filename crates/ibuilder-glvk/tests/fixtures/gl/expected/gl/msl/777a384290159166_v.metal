#include <metal_stdlib>
#include <simd/simd.h>

using namespace metal;

struct _RESERVED_IDENTIFIER_FIXUP_gl_DefaultUniformBlock
{
    float4x4 u_matrix;
};

struct spvDescriptorSetBuffer0
{
    constant _RESERVED_IDENTIFIER_FIXUP_gl_DefaultUniformBlock* dm_gl_resource_25 [[id(0)]];
};

struct gl_777a384290159166_v_out
{
    float2 v_uv [[user(v_uv)]];
    float4 gl_Position [[position]];
};

struct gl_777a384290159166_v_in
{
    float4 a_position [[attribute(0)]];
    float2 a_texcoord [[attribute(1)]];
};

vertex gl_777a384290159166_v_out gl_777a384290159166_v(gl_777a384290159166_v_in in [[stage_in]], constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]])
{
    gl_777a384290159166_v_out out = {};
    float2 gm_sample = in.a_texcoord;
    out.v_uv = gm_sample;
    out.gl_Position = (*spvDescriptorSet0.dm_gl_resource_25).u_matrix * in.a_position;
    out.gl_Position.z = (out.gl_Position.z + out.gl_Position.w) * 0.5;       // Adjust clip-space for Metal
    return out;
}

