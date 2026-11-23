#include <metal_stdlib>
#include <simd/simd.h>

using namespace metal;

struct gl_30da25c68ea3b458_v_out
{
    float4 gl_Position [[position]];
};

struct gl_30da25c68ea3b458_v_in
{
    float4 position [[attribute(0)]];
};

vertex gl_30da25c68ea3b458_v_out gl_30da25c68ea3b458_v(gl_30da25c68ea3b458_v_in in [[stage_in]])
{
    gl_30da25c68ea3b458_v_out out = {};
    out.gl_Position = in.position;
    out.gl_Position.z = (out.gl_Position.z + out.gl_Position.w) * 0.5;       // Adjust clip-space for Metal
    return out;
}

