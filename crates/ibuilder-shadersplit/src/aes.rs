//! AES (FIPS-197) for the readers of encrypted containers: AES-256 ECB decryption, what the engine's `dm/tools/aes.py` `decrypt(key, data)` does through CommonCrypto for the
//! Unreal readers (an encrypted pak index, encrypted pak entries, encrypted IoStore blocks and directory indices) and the RPF6 reader (its tables and scripts), and AES-128 CBC for
//! EA's EASF files (`easf.rs`; encryption too, for the tests' fixtures). No padding is removed by the decryptors. A plain table implementation (the equivalent inverse cipher with a
//! 32-bit table): the crate forbids `unsafe`, so no CPU AES instructions. Tables are computed at compile time from the field arithmetic, none is typed in. Keys are never printed or
//! written by the callers.

const fn xt(a: u8) -> u8 {
    (a << 1) ^ if a & 0x80 != 0 { 0x1b } else { 0 }
}

/// Multiplication in GF(2^8)
const fn mul(mut a: u8, mut b: u8) -> u8 {
    let mut r = 0;
    while b != 0 {
        if b & 1 != 0 {
            r ^= a;
        }
        a = xt(a);
        b >>= 1;
    }
    r
}

/// The forward S-box: the multiplicative inverse (x^254), then the affine map
const SBOX: [u8; 256] = {
    let mut s = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        let x = i as u8;
        let mut inv = if i == 0 { 0 } else { 1u8 };
        let mut k = 0;
        while i != 0 && k < 254 {
            inv = mul(inv, x);
            k += 1;
        }
        s[i] = inv
            ^ inv.rotate_left(1)
            ^ inv.rotate_left(2)
            ^ inv.rotate_left(3)
            ^ inv.rotate_left(4)
            ^ 0x63;
        i += 1;
    }
    s
};

const INV: [u8; 256] = {
    let mut v = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        v[SBOX[i] as usize] = i as u8;
        i += 1;
    }
    v
};

/// Td0: InvSubBytes followed by the InvMixColumns column of one byte (rows as the bytes of a big-endian word); Td1..Td3 are its rotations
const TD0: [u32; 256] = {
    let mut t = [0u32; 256];
    let mut i = 0;
    while i < 256 {
        let s = INV[i];
        t[i] = (mul(s, 14) as u32) << 24
            | (mul(s, 9) as u32) << 16
            | (mul(s, 13) as u32) << 8
            | mul(s, 11) as u32;
        i += 1;
    }
    t
};

/// InvMixColumns of one big-endian column word (for the equivalent inverse key schedule)
fn inv_mix(w: u32) -> u32 {
    let b = w.to_be_bytes();
    let c = |p, q, r, s| mul(b[0], p) ^ mul(b[1], q) ^ mul(b[2], r) ^ mul(b[3], s);
    u32::from_be_bytes([
        c(14, 11, 13, 9),
        c(9, 14, 11, 13),
        c(13, 9, 14, 11),
        c(11, 13, 9, 14),
    ])
}

fn sub_word(w: u32) -> u32 {
    let b = w.to_be_bytes();
    u32::from_be_bytes([
        SBOX[b[0] as usize],
        SBOX[b[1] as usize],
        SBOX[b[2] as usize],
        SBOX[b[3] as usize],
    ])
}

/// An expanded key of 4 or 8 words: the cipher's round keys and the equivalent inverse cipher's (big-endian column words, at most 15 rounds)
struct Schedule {
    ek: [u32; 60],
    dk: [u32; 60],
    nr: usize,
}

impl Schedule {
    fn new(key: &[u8]) -> Schedule {
        let nk = key.len() / 4;
        let nr = nk + 6;
        let mut w = [0u32; 60];
        for (i, c) in key.chunks(4).enumerate() {
            w[i] = u32::from_be_bytes(c.try_into().unwrap());
        }
        let mut rcon = 1u8;
        for i in nk..4 * (nr + 1) {
            let mut t = w[i - 1];
            if i % nk == 0 {
                t = sub_word(t.rotate_left(8)) ^ ((rcon as u32) << 24);
                rcon = xt(rcon);
            } else if nk > 6 && i % nk == 4 {
                t = sub_word(t);
            }
            w[i] = w[i - nk] ^ t;
        }
        // dk round r = ek round nr - r, InvMixColumns applied to rounds 1..nr-1
        let mut dk = [0u32; 60];
        for r in 0..=nr {
            for c in 0..4 {
                let e = w[4 * (nr - r) + c];
                dk[4 * r + c] = if r == 0 || r == nr { e } else { inv_mix(e) };
            }
        }
        Schedule { ek: w, dk, nr }
    }

    /// One 16-byte block in place
    fn decrypt_block(&self, b: &mut [u8]) {
        let k = &self.dk;
        let mut s = [0u32; 4];
        for c in 0..4 {
            s[c] = u32::from_be_bytes(b[4 * c..4 * c + 4].try_into().unwrap()) ^ k[c];
        }
        let td = |x: u32, rot: u32| TD0[x as usize & 0xff].rotate_right(rot);
        for r in 1..self.nr {
            let mut t = [0u32; 4];
            for c in 0..4 {
                // InvShiftRows: row j of column c comes from column c - j
                t[c] = td(s[c] >> 24, 0)
                    ^ td(s[(c + 3) % 4] >> 16, 8)
                    ^ td(s[(c + 2) % 4] >> 8, 16)
                    ^ td(s[(c + 1) % 4], 24)
                    ^ k[4 * r + c];
            }
            s = t;
        }
        for c in 0..4 {
            let v = u32::from_be_bytes([
                INV[(s[c] >> 24) as usize],
                INV[(s[(c + 3) % 4] >> 16) as usize & 0xff],
                INV[(s[(c + 2) % 4] >> 8) as usize & 0xff],
                INV[s[(c + 1) % 4] as usize & 0xff],
            ]) ^ k[4 * self.nr + c];
            b[4 * c..4 * c + 4].copy_from_slice(&v.to_be_bytes());
        }
    }

    /// One 16-byte block in place (the state is column-major: byte 4c+r is row r of column c); the readers only decrypt, the tests encrypt their fixtures
    fn encrypt_block(&self, b: &mut [u8]) {
        let add = |b: &mut [u8], r: usize| {
            for c in 0..4 {
                let k = self.ek[4 * r + c].to_be_bytes();
                for row in 0..4 {
                    b[4 * c + row] ^= k[row];
                }
            }
        };
        add(b, 0);
        for r in 1..=self.nr {
            for x in b.iter_mut() {
                *x = SBOX[*x as usize];
            }
            let s: [u8; 16] = b[..16].try_into().unwrap();
            for c in 0..4 {
                for row in 0..4 {
                    b[4 * c + row] = s[4 * ((c + row) % 4) + row];
                }
            }
            if r < self.nr {
                for c in 0..4 {
                    let a = [b[4 * c], b[4 * c + 1], b[4 * c + 2], b[4 * c + 3]];
                    for row in 0..4 {
                        b[4 * c + row] = mul(a[row], 2)
                            ^ mul(a[(row + 1) % 4], 3)
                            ^ a[(row + 2) % 4]
                            ^ a[(row + 3) % 4];
                    }
                }
            }
            add(b, r);
        }
    }
}

/// An AES-128 key (EASF)
pub struct Aes128(Schedule);

impl Aes128 {
    pub fn new(key: &[u8; 16]) -> Aes128 {
        Aes128(Schedule::new(key))
    }

    /// one block, in place
    pub fn encrypt_block(&self, b: &mut [u8; 16]) {
        self.0.encrypt_block(b);
    }

    /// one block, in place
    pub fn decrypt_block(&self, b: &mut [u8; 16]) {
        self.0.decrypt_block(b);
    }

    /// CBC decryption of whole blocks (a partial last block is left out)
    pub fn cbc_decrypt(&self, iv: &[u8; 16], data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len());
        let mut prev = *iv;
        for ch in data.as_chunks::<16>().0 {
            let mut b = *ch;
            self.decrypt_block(&mut b);
            for (x, p) in b.iter_mut().zip(&prev) {
                *x ^= p;
            }
            prev = *ch;
            out.extend_from_slice(&b);
        }
        out
    }

    /// CBC encryption with PKCS#7 padding (the tests make EASF fixtures with it)
    pub fn cbc_encrypt_pkcs7(&self, iv: &[u8; 16], data: &[u8]) -> Vec<u8> {
        let pad = 16 - data.len() % 16;
        let mut d = data.to_vec();
        d.extend(std::iter::repeat_n(pad as u8, pad));
        let mut prev = *iv;
        for ch in d.as_chunks_mut::<16>().0 {
            let mut b = *ch;
            for (x, p) in b.iter_mut().zip(&prev) {
                *x ^= p;
            }
            self.encrypt_block(&mut b);
            ch.copy_from_slice(&b);
            prev = b;
        }
        d
    }
}

/// An AES-256 key (Unreal paks and IoStore containers, RPF6)
pub struct Aes256(Schedule);

impl Aes256 {
    pub fn new(key: &[u8; 32]) -> Aes256 {
        Aes256(Schedule::new(key))
    }

    /// one 16-byte block, in place
    pub fn decrypt_block(&self, b: &mut [u8]) {
        self.0.decrypt_block(b);
    }

    /// one 16-byte block, in place (the tests' fixtures)
    pub fn encrypt_block(&self, b: &mut [u8]) {
        self.0.encrypt_block(b);
    }

    /// ECB over whole blocks, in place (a tail of fewer than 16 bytes is left as it is)
    pub fn decrypt(&self, d: &mut [u8]) {
        for b in d.as_chunks_mut::<16>().0 {
            self.decrypt_block(b);
        }
    }
}

/// `aes.decrypt(key, data)` (AES-256 ECB): `Err` = the Python's ValueError text when the data is not whole blocks
pub fn decrypt(key: &[u8; 32], data: &[u8]) -> Result<Vec<u8>, String> {
    if !data.len().is_multiple_of(16) {
        return Err("invalid AES key, data or IV length".into());
    }
    let mut out = data.to_vec();
    Aes256::new(key).decrypt(&mut out);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unhex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    fn hex(b: &[u8]) -> String {
        b.iter().map(|x| format!("{x:02x}")).collect()
    }

    #[test]
    fn sbox_known_entries() {
        assert_eq!(
            (SBOX[0x00], SBOX[0x01], SBOX[0x53], SBOX[0xff]),
            (0x63, 0x7c, 0xed, 0x16)
        );
        assert!((0..256).all(|i| INV[SBOX[i] as usize] as usize == i));
    }

    #[test]
    fn fips_197_aes_128() {
        // FIPS-197 appendix C.1
        let key: [u8; 16] = std::array::from_fn(|i| i as u8);
        let a = Aes128::new(&key);
        let mut b: [u8; 16] = unhex("00112233445566778899aabbccddeeff")
            .try_into()
            .unwrap();
        a.encrypt_block(&mut b);
        assert_eq!(hex(&b), "69c4e0d86a7b0430d8cdb78070b4c55a");
        a.decrypt_block(&mut b);
        assert_eq!(hex(&b), "00112233445566778899aabbccddeeff");
        // FIPS-197 appendix A.1: the last round key of 2b7e1516...
        let s = Schedule::new(&unhex("2b7e151628aed2a6abf7158809cf4f3c"));
        assert_eq!(
            &s.ek[40..44],
            &[0xd014f9a8, 0xc9ee2589, 0xe13f0cc8, 0xb6630ca6]
        );
    }

    #[test]
    fn sp_800_38a_cbc_aes_128() {
        // SP 800-38A F.2.1 / F.2.2 (CBC-AES128), the first two blocks
        let key: [u8; 16] = unhex("2b7e151628aed2a6abf7158809cf4f3c")
            .try_into()
            .unwrap();
        let iv: [u8; 16] = unhex("000102030405060708090a0b0c0d0e0f")
            .try_into()
            .unwrap();
        let ct = unhex("7649abac8119b246cee98e9b12e9197d5086cb9b507219ee95db113a917678b2");
        let pt = "6bc1bee22e409f96e93d7e117393172aae2d8a571e03ac9c9eb76fac45af8e51";
        let a = Aes128::new(&key);
        assert_eq!(hex(&a.cbc_decrypt(&iv, &ct)), pt);
        let e = a.cbc_encrypt_pkcs7(&iv, &unhex(pt));
        assert_eq!(e.len(), 48);
        assert_eq!(&e[..32], ct.as_slice());
        assert_eq!(&a.cbc_decrypt(&iv, &e)[32..], &[16u8; 16]);
    }

    #[test]
    fn fips_197_aes_256() {
        // FIPS-197 appendix C.3
        let key: [u8; 32] = std::array::from_fn(|i| i as u8);
        let ct = unhex("8ea2b7ca516745bfeafc49904b496089");
        let pt = decrypt(&key, &ct).unwrap();
        assert_eq!(hex(&pt), "00112233445566778899aabbccddeeff");
        let mut b = pt.clone();
        Aes256::new(&key).encrypt_block(&mut b);
        assert_eq!(b, ct);
        assert!(decrypt(&key, &ct[..15]).is_err());
        // a tail is left as it is
        let mut d = [ct.clone(), vec![7u8; 5]].concat();
        Aes256::new(&key).decrypt(&mut d);
        assert_eq!(&d[..16], pt.as_slice());
        assert_eq!(&d[16..], &[7u8; 5]);
    }

    #[test]
    fn sp_800_38a_ecb_aes_256() {
        // SP 800-38A F.1.6 (ECB-AES256.Decrypt), the four blocks
        let key: [u8; 32] =
            unhex("603deb1015ca71be2b73aef0857d77811f352c073b6108d72d9810a30914dff4")
                .try_into()
                .unwrap();
        let ct = unhex(concat!(
            "f3eed1bdb5d2a03c064b5a7e3db181f8",
            "591ccb10d410ed26dc5ba74a31362870",
            "b6ed21b99ca6f4f9f153e7b1beafed1d",
            "23304b7a39f9f3ff067d8d8f9e24ecc7"
        ));
        let pt = concat!(
            "6bc1bee22e409f96e93d7e117393172a",
            "ae2d8a571e03ac9c9eb76fac45af8e51",
            "30c81c46a35ce411e5fbc1191a0a52ef",
            "f69f2445df4f9b17ad2b417be66c3710"
        );
        assert_eq!(hex(&decrypt(&key, &ct).unwrap()), pt);
    }
}
