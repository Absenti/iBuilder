#include <metal_stdlib>
#include <simd/simd.h>

using namespace metal;

struct gl_7b0ad52a20755ac7_f_out
{
    float4 gm_FragColor [[color(0)]];
};

struct gl_7b0ad52a20755ac7_f_in
{
    float4 v_color [[user(v_color)]];
};

fragment gl_7b0ad52a20755ac7_f_out gl_7b0ad52a20755ac7_f(gl_7b0ad52a20755ac7_f_in in [[stage_in]])
{
    gl_7b0ad52a20755ac7_f_out out = {};
    out.gm_FragColor = in.v_color;
    return out;
}

