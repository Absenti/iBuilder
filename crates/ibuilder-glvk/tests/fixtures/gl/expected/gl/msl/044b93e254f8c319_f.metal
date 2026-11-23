#pragma clang diagnostic ignored "-Wmissing-prototypes"

#include <metal_stdlib>
#include <simd/simd.h>

using namespace metal;

struct _RESERVED_IDENTIFIER_FIXUP_gl_DefaultUniformBlock
{
    float4 _fa_[2];
};

struct spvDescriptorSetBuffer0
{
    texture2d<float> dm_gl_resource_25 [[id(0)]];
    sampler dm_gl_resource_25Smplr [[id(1)]];
    constant _RESERVED_IDENTIFIER_FIXUP_gl_DefaultUniformBlock* dm_gl_resource_43 [[id(2)]];
};

struct gl_044b93e254f8c319_f_out
{
    float4 dm_FragColor [[color(0)]];
};

struct gl_044b93e254f8c319_f_in
{
    float4 dm_Color [[user(dm_Color)]];
    float2 vofi_TexCoord0 [[user(vofi_TexCoord0)]];
};

static inline __attribute__((always_inline))
float4 tex2D(texture2d<float> dm_sampler, sampler dm_samplerSmplr, thread const float2& texcoord)
{
    return dm_sampler.sample(dm_samplerSmplr, texcoord);
}

fragment gl_044b93e254f8c319_f_out gl_044b93e254f8c319_f(gl_044b93e254f8c319_f_in in [[stage_in]], constant spvDescriptorSetBuffer0& spvDescriptorSet0 [[buffer(0)]], float4 gl_FragCoord [[position]])
{
    gl_044b93e254f8c319_f_out out = {};
    float2 param = in.vofi_TexCoord0;
    float4 c = tex2D(spvDescriptorSet0.dm_gl_resource_25, spvDescriptorSet0.dm_gl_resource_25Smplr, param) * in.dm_Color;
    out.dm_FragColor = (c * (*spvDescriptorSet0.dm_gl_resource_43)._fa_[0]) + ((*spvDescriptorSet0.dm_gl_resource_43)._fa_[1] * gl_FragCoord.x);
    return out;
}

