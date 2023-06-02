pub mod crc;
pub mod parser;
pub use parser::Parser;
#[cfg(all(feature = "codec"))]
pub mod codec;
#[cfg(feature = "codec")]
pub use codec::{Decoder, Encoder};

pub const STX: [u8; 2] = [0xeb, 0x90];
pub const ETX: [u8; 2] = [0xc5, 0x79];
pub const LEN_SIZE: usize = 2;
pub const CRC_SIZE: usize = 2;
pub const HEADER_SIZE: usize = STX.len() + LEN_SIZE;
pub const FOOTER_SIZE: usize = CRC_SIZE + ETX.len();
