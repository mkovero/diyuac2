//! CRC-32 (IEEE 802.3, as zlib and Python's `zlib.crc32`), nibble table.

const TABLE: [u32; 16] = {
    let mut t = [0u32; 16];
    let mut i = 0;
    while i < 16 {
        let mut c = i as u32;
        let mut k = 0;
        while k < 4 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
            k += 1;
        }
        t[i] = c;
        i += 1;
    }
    t
};

/// Continue a CRC: start with `crc = 0`.
pub fn update(crc: u32, data: &[u8]) -> u32 {
    let mut c = !crc;
    for &b in data {
        c = TABLE[((c ^ b as u32) & 0xF) as usize] ^ (c >> 4);
        c = TABLE[((c ^ (b as u32 >> 4)) & 0xF) as usize] ^ (c >> 4);
    }
    !c
}
