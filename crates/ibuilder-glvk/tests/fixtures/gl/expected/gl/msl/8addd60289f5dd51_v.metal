#include <metal_stdlib>
#include <simd/simd.h>

using namespace metal;

struct DirLights
{
    float3 dir[2];
    float4 color;
};

struct LightEnv
{
    DirLights m_dirLights;
    float m_ambient;
};

struct _RESERVED_IDENTIFIER_FIXUP_gl_DefaultUniformBlock
{
    LightEnv u_LightEnv;
    float4 u_weights[4];
    uint u_on;
    float u_unused;
};

struct spvDescriptorSetBuffer0
{
    constant _RESERVED_IDENTIFIER_FIXUP_gl_DefaultUniformBlock* dm_gl_resource_23 [[id(0)]];
};

struct gl_8addd60289f5dd51_v_out
{
    float4 v_col [[user(v_col)]];
    float v_w [[user(v_w)]];
    float4 gl_Position [[position]];
};

struct gl_8addd60289f5dd51_v_in
{
    float3 a_pos [[attribute(0)]];
    float3 a_nrm [[attribute(1)]];
};

vertex gl_8addd60289f5dd51_v_out gl_8addd60289f5dd51_v(gl_8addd60289f5dd51_v_in in [[stage_in]], constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]])
{
    gl_8addd60289f5dd51_v_out out = {};
    float l = precise::max(dot(in.a_nrm, (*spvDescriptorSet0.dm_gl_resource_23).u_LightEnv.m_dirLights.dir[0]), 0.0) + (*spvDescriptorSet0.dm_gl_resource_23).u_LightEnv.m_ambient;
    float4 _47;
    if ((*spvDescriptorSet0.dm_gl_resource_23).u_on != 0u)
    {
        _47 = (*spvDescriptorSet0.dm_gl_resource_23).u_LightEnv.m_dirLights.color * l;
    }
    else
    {
        _47 = float4(l);
    }
    out.v_col = _47;
    out.v_w = (*spvDescriptorSet0.dm_gl_resource_23).u_weights[0].x + (*spvDescriptorSet0.dm_gl_resource_23).u_weights[3].x;
    out.gl_Position = float4(in.a_pos, 1.0);
    out.gl_Position.z = (out.gl_Position.z + out.gl_Position.w) * 0.5;       // Adjust clip-space for Metal
    return out;
}

