#include <metal_stdlib>
#include <simd/simd.h>

using namespace metal;

struct _RESERVED_IDENTIFIER_FIXUP_gl_DefaultUniformBlock
{
    float4x4 mvp;
};

struct spvDescriptorSetBuffer0
{
    constant _RESERVED_IDENTIFIER_FIXUP_gl_DefaultUniformBlock* dm_gl_resource_34 [[id(0)]];
};

struct gl_1a592aedeb3b04e4_v_out
{
    float4 dm_LegacyColor [[user(dm_LegacyColor)]];
    float4 dm_LegacyTexCoord0 [[user(dm_LegacyTexCoord0)]];
    float4 gl_Position [[position]];
};

struct gl_1a592aedeb3b04e4_v_in
{
    float3 pos [[attribute(0)]];
    float2 uv [[attribute(1)]];
    float4 col [[attribute(2)]];
};

vertex gl_1a592aedeb3b04e4_v_out gl_1a592aedeb3b04e4_v(gl_1a592aedeb3b04e4_v_in in [[stage_in]], constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]])
{
    gl_1a592aedeb3b04e4_v_out out = {};
    out.dm_LegacyTexCoord0 = float4(in.uv, 0.0, 1.0);
    out.dm_LegacyColor = in.col;
    out.gl_Position = (*spvDescriptorSet0.dm_gl_resource_34).mvp * float4(in.pos, 1.0);
    out.gl_Position.z = (out.gl_Position.z + out.gl_Position.w) * 0.5;       // Adjust clip-space for Metal
    return out;
}

