pub const ALGO: crc::Crc<u16> = crc::Crc::<u16>::new(&crc::CRC_16_ARC);

#[cfg(test)]
mod tests {
    use super::*;
    const DEADBEEF: &[u8] = &[0xde, 0xad, 0xbe, 0xef];

    #[test]
    fn test_crc() {
        let input = DEADBEEF;
        let crc = ALGO.checksum(input).to_be_bytes();
        assert_eq!(crc, [0xe5, 0x9b]);
    }
}
