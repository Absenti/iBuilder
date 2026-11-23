#include <metal_stdlib>
#include <simd/simd.h>

using namespace metal;

struct matrices_ubo
{
    float4 matrices[8];
};

struct spvDescriptorSetBuffer0
{
    constant matrices_ubo* dm_gl_resource_23 [[id(0)]];
};

struct gl_cde7362aed88dcf2_v_out
{
    float4 dm_Color [[user(dm_Color)]];
    float2 vofi_TexCoord0 [[user(vofi_TexCoord0)]];
    float4 gl_Position [[position]];
};

struct gl_cde7362aed88dcf2_v_in
{
    float4 attr_Vertex [[attribute(0)]];
    float4 attr_Color [[attribute(1)]];
};

vertex gl_cde7362aed88dcf2_v_out gl_cde7362aed88dcf2_v(gl_cde7362aed88dcf2_v_in in [[stage_in]], constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]])
{
    gl_cde7362aed88dcf2_v_out out = {};
    out.gl_Position.x = dot(in.attr_Vertex, (*spvDescriptorSet0.dm_gl_resource_23).matrices[0]);
    out.gl_Position.y = dot(in.attr_Vertex, (*spvDescriptorSet0.dm_gl_resource_23).matrices[1]);
    out.gl_Position.z = dot(in.attr_Vertex, (*spvDescriptorSet0.dm_gl_resource_23).matrices[2]);
    out.gl_Position.w = dot(in.attr_Vertex, (*spvDescriptorSet0.dm_gl_resource_23).matrices[3]);
    out.dm_Color = in.attr_Color;
    out.vofi_TexCoord0 = in.attr_Vertex.xy;
    out.gl_Position.z = (out.gl_Position.z + out.gl_Position.w) * 0.5;       // Adjust clip-space for Metal
    return out;
}

