#include <metal_stdlib>
#include <simd/simd.h>

using namespace metal;

struct gl_a8ce72e1394a1499_v_out
{
    float4 gl_Position [[position]];
};

struct gl_a8ce72e1394a1499_v_in
{
    float4 p [[attribute(0)]];
};

vertex gl_a8ce72e1394a1499_v_out gl_a8ce72e1394a1499_v(gl_a8ce72e1394a1499_v_in in [[stage_in]])
{
    gl_a8ce72e1394a1499_v_out out = {};
    out.gl_Position = in.p;
    out.gl_Position.z = (out.gl_Position.z + out.gl_Position.w) * 0.5;       // Adjust clip-space for Metal
    return out;
}

