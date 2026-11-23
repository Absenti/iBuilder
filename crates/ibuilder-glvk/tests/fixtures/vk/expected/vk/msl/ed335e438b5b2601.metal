#include <metal_stdlib>
#include <simd/simd.h>

using namespace metal;

struct Unused
{
    float4 never;
};

struct vk_ed335e438b5b2601_out
{
    float4 o0 [[color(0)]];
    float4 o1 [[color(1)]];
};

struct vk_ed335e438b5b2601_in
{
    float2 uv [[user(locn0)]];
};

fragment vk_ed335e438b5b2601_out vk_ed335e438b5b2601(vk_ed335e438b5b2601_in in [[stage_in]], texture2d<float> t [[texture(0)]], texture2d<float> cs [[texture(1)]], sampler s [[sampler(0)]], sampler csSmplr [[sampler(1)]])
{
    vk_ed335e438b5b2601_out out = {};
    out.o0 = precise::max(t.sample(s, in.uv), float4(0.25));
    out.o1 = cs.sample(csSmplr, in.uv.yx);
    return out;
}

