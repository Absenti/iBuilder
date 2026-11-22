//! A load command that cannot advance (review r2, N2): `ibuilder-macho`'s `parse` already refused `cmdsize < 8` and `ncmds * 8 > sizeofcmds`; the walk of this crate did not, and pushed one 16-byte
//! entry per command for `ncmds` (up to 2^32) turns. The cases with a count the old walk could still finish come FIRST, so that on the old code the test fails there and never reaches `u32::MAX`.
use ibuilder_sign::verify_bytes;

fn header(ncmds: u32, sizeofcmds: u32, cmds: &[u8]) -> Vec<u8> {
    let mut f = Vec::new();
    for v in [0xFEED_FACFu32, 0x0100_000C, 0, 6, ncmds, sizeofcmds, 0, 0] {
        f.extend_from_slice(&v.to_le_bytes());
    }
    f.extend_from_slice(cmds);
    f
}

#[test]
fn load_commands_that_cannot_advance_or_cannot_fit_are_errors_not_a_loop() {
    let zero_size = [0x19u8, 0, 0, 0, 0, 0, 0, 0]; // LC_SEGMENT_64 with cmdsize 0: `off` never moves
    let cmds = [zero_size.as_slice(); 3].concat();
    for (ncmds, sizeofcmds) in [
        (10_000_000u32, 8u32),
        (10_000_000, 80_000_000),
        (1, 8),
        (3, 24),
        (u32::MAX, 8),
        (u32::MAX, u32::MAX),
    ] {
        let e = verify_bytes(&header(ncmds, sizeofcmds, &cmds))
            .unwrap_err()
            .to_string();
        assert!(
            e.contains("cannot fit")
                || e.contains("smaller than its own 8-byte header")
                || e.contains("refused"),
            "ncmds {ncmds} sizeofcmds {sizeofcmds}: the commands were walked first: {e}"
        );
    }
}
