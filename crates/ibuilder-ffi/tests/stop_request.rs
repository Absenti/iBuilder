//! `ibuilder_request_stop` / `ibuilder_stop_requested` at the C boundary, in a process of their own (the request is one byte of the process; the whole chain through the ABI is
//! `tests/c/test_abi.c`, run by `c_abi.rs`).

use ibuilder_ffi::{ibuilder_cancel, ibuilder_request_stop, ibuilder_stop_requested};

#[test]
fn a_pause_and_a_cancel_are_requested_overridden_and_withdrawn_through_the_c_functions() {
    assert_eq!(ibuilder_stop_requested(), 0);
    assert_eq!(ibuilder_request_stop(1), 1, "a pause");
    assert_eq!(ibuilder_stop_requested(), 1);
    assert_eq!(ibuilder_request_stop(2), 2, "a cancel overrides a pause");
    assert_eq!(
        ibuilder_request_stop(1),
        2,
        "a pause does not override a cancel"
    );
    assert_eq!(ibuilder_stop_requested(), 2);
    for withdraw in [0, 3, 7, -1, -2147483648, 257, 258, i32::MAX] {
        assert_eq!(ibuilder_request_stop(2), 2);
        assert_eq!(
            ibuilder_request_stop(withdraw),
            0,
            "{withdraw} withdraws (257 and 258 are not 1 and 2 as bytes)"
        );
        assert_eq!(ibuilder_stop_requested(), 0);
    }
    // a cancel by handle with no handle is a no-op, and leaves what was asked as it was
    assert_eq!(ibuilder_request_stop(1), 1);
    // SAFETY: NULL is documented as a no-op.
    unsafe { ibuilder_cancel(std::ptr::null_mut()) };
    assert_eq!(ibuilder_stop_requested(), 1);
    assert_eq!(ibuilder_request_stop(0), 0);
}
