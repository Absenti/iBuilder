#include <metal_stdlib>
#include <simd/simd.h>

using namespace metal;

struct Out
{
    float v[1];
};

constant uint3 gl_WorkGroupSize [[maybe_unused]] = uint3(8u, 4u, 1u);

kernel void vk_4b736327bc15ce89(device Out& outb [[buffer(0)]], texture_buffer<float> src [[texture(0)]], texture_buffer<float, access::write> dst [[texture(1)]], uint3 gl_GlobalInvocationID [[thread_position_in_grid]])
{
    uint i = gl_GlobalInvocationID.x;
    float x = src.read(uint(int(i))).x;
    outb.v[i] = precise::clamp(precise::min(x, 4.0), 0.0, 1.0);
    dst.write(float4(x), uint(int(i)));
}

