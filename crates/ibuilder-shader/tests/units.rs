//! Unit tests: hand-made inputs whose expected values were produced by running the engine's Python on the same text (c1_guard.Interval, definitions_of, the rewrite steps).
use ibuilder_shader::interval::Interval;
use ibuilder_shader::tables::definitions_of;
use ibuilder_shader::{steps, text};

#[test]
fn interval_matches_the_python() {
    // the source and the (low, high) of each expression come from c1_guard.Interval(definitions_of(src)).expression(e) on the Python 3.14 reference
    let src = "\nint _1 = as_type<int>(in.x) & 255;\nuint _2 = uint(_1) >> 2u;\nuint _3 = _2 * 4u + 3u;\nint _4 = int(gl_x);\nbool _5 = _3 < 100u;\nint _6 = _5 ? _1 : 7;\nuint _7 = min(_3, 20u);\nint _8 = clamp(_4, 0, 15);\nuint _9 = as_type<uint>(_4) % 16u;\nint _10 = _1 << 2;\nint _11 = (_1 | 3) ^ 1;\nint _12 = -_1 + 10;\nint _13 = _13 + 1;\nint _14 = 010;\n";
    // table rows are kept one per line as written (rustfmt would put every field of every row on its own line)
    #[rustfmt::skip]
    let rows: &[(&str, Option<i128>, Option<i128>)] = &[
        ("_1", Some(0), Some(255)), ("_2", Some(0), Some(63)), ("_3", Some(3), Some(255)), ("_4", None, None), ("_5", Some(0), Some(1)), ("_6", Some(0), Some(255)),
        ("_7", Some(3), Some(20)), ("_8", Some(0), Some(15)), ("_9", Some(0), Some(15)), ("_10", Some(0), Some(1020)), ("_11", Some(0), Some(255)), ("_12", Some(-245), Some(10)),
        ("_13", None, None), ("_14", None, None), ("_1 + _2", Some(0), Some(318)), ("_3 * 2u - 1", Some(5), Some(509)), ("(_2 + 1) * 3", Some(3), Some(192)), ("_7 / 4u", Some(0), Some(5)),
        ("_8 + 1", Some(1), Some(16)), ("0x1Fu", Some(31), Some(31)), ("7", Some(7), Some(7)), ("3.5", None, None), ("_1 ? 1 : 2", Some(1), Some(2)), ("_5 ? _2 : _7", Some(0), Some(63)),
        ("max(_2, _7)", Some(3), Some(63)), ("_2 >> 1", Some(0), Some(31)), ("foo(1)", None, None), ("_99", None, None), ("_1 +", None, None), ("(_1", None, None),
        ("as_type<uint>(_4)", Some(0), None), ("~_1", None, None), ("!_1", Some(0), Some(1)), ("_1[0]", None, None), ("_1.x", None, None), ("uint(_4)", Some(0), None),
        ("int(_2) - 100", Some(-100), Some(-37)), ("(_1 & 15) + (_2 & 7) * 16", Some(0), Some(127)), ("_3 % 5u", Some(0), Some(4)), ("_8 % 3", Some(0), Some(2)),
        ("1 << 40", Some(1099511627776), Some(1099511627776)), ("100000000000 * 100000000000", Some(10000000000000000000000), Some(10000000000000000000000)),
    ];
    let defs = definitions_of(src);
    for (e, lo, hi) in rows {
        let (l, h) = Interval::new(&defs).expression(e);
        assert_eq!((l, h), (*lo, *hi), "expression {e}");
    }
}

#[test]
fn precise_rewrites_only_whole_fast_calls() {
    assert_eq!(
        steps::precise(
            "a = fast::min(x, y); b = xfast::max(1) + fast::clamp(z, 0.0, 1.0); fast::mix(1)"
        ),
        "a = precise::min(x, y); b = xfast::max(1) + precise::clamp(z, 0.0, 1.0); fast::mix(1)"
    );
    assert!(matches!(
        steps::precise("nothing here"),
        std::borrow::Cow::Borrowed(_)
    ));
}

#[test]
fn membar_fence_dedupes_a_pair_and_leaves_the_third() {
    let m = ibuilder_shader::templates::MEMBAR;
    let f = ibuilder_shader::templates::MEMBAR_FENCE;
    let src = format!("  {m}\n  {m}\n  {m}\nx\n{m}\n");
    // the first two lines are one match (-> one barrier); the third is alone (a barrier); then a lone one at column 0; every barrier becomes the fence
    assert_eq!(steps::membar_fence(&src), format!("  {f}\n  {f}\nx\n{f}\n"));
}

#[test]
fn text_helpers() {
    assert_eq!(text::close(b"a(b(c)d)e", 2, b'(', b')'), Some(8));
    assert_eq!(text::block_end(b"{ { } }", 0), Some(6));
    assert_eq!(text::block_end(b"xyz", 1), Some(1));
}

#[test]
fn compute_loops_contains_a_compact_loop() {
    // dmsh_rewrite.compute_loops' site dicts are {'site', 'function', 'line', 'barrier'} (the reflection writes them in that order, finish/mod.rs)
    let src = "using namespace metal;\nkernel void main0()\n{\n    for (;;) x++;\n}\n";
    let (out, sites) = ibuilder_shader::cs::compute_loops(src).unwrap();
    // the tail as dmsh_rewrite.compute_loops writes it for the same text
    assert!(out.ends_with("kernel void main0(device dm_loop_state& dm_loop [[buffer(28)]])\n{\n    uint dm_lg = 0u;\n    for (;(true) && dm_lg_go(dm_lg, dm_loop, 1u) /* dm_loop_site 1: main0 */;) x++;\n}\n"));
    assert_eq!(
        sites[0].to_json(),
        serde_json::json!({"site": 1, "function": "main0", "line": 4, "barrier": false})
    );
}

#[test]
fn raw_noinline_follows_the_stage() {
    let src = "x\nstatic inline __attribute__((always_inline))\nuint dmsh_raw_find(uint a)\n";
    let cs = steps::raw_noinline(src, "cs");
    assert_eq!(
        cs,
        "x\nstatic __attribute__((noinline))\nuint dmsh_raw_find(uint a)\n"
    );
    assert_eq!(steps::raw_noinline(&cs, "ms"), src);
}
