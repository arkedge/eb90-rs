use bytes::{Bytes, BytesMut};

use crate::{
    crc,
    parser::{self, Error, JunkKind, Parser},
    ETX, STX,
};

pub struct Encoder(());

impl Encoder {
    #[allow(clippy::new_without_default)]
    pub fn new() -> Self {
        Self(())
    }
}

impl<T> tokio_util::codec::Encoder<T> for Encoder
where
    T: AsRef<[u8]>,
{
    type Error = std::io::Error;

    fn encode(&mut self, item: T, dst: &mut BytesMut) -> Result<(), Self::Error> {
        let bytes = item.as_ref();
        dst.extend_from_slice(&STX);
        if bytes.len() > u16::MAX as usize {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("body length {} is too large.", bytes.len()),
            ));
        }
        dst.extend_from_slice(&(bytes.len() as u16).to_be_bytes());
        dst.extend_from_slice(bytes);
        let checksum = crc::ALGO.checksum(bytes);
        dst.extend_from_slice(&checksum.to_be_bytes());
        dst.extend_from_slice(&ETX);
        Ok(())
    }
}

pub struct Decoder<B> {
    parser: Parser<B>,
}

impl<B> Decoder<B>
where
    B: parser::Buffer,
{
    pub fn new(buf: B) -> Self {
        let parser = Parser::with_buffer(buf);
        Self { parser }
    }
}

impl<B> tokio_util::codec::Decoder for Decoder<B>
where
    B: parser::Buffer,
{
    type Item = Decoded;
    type Error = std::io::Error;

    fn decode(&mut self, src: &mut BytesMut) -> Result<Option<Self::Item>, Self::Error> {
        let filled_len = self.parser.fill(src);
        let _ = src.split_to(filled_len);
        match self.parser.read() {
            Err(Error::Incomplete) => Ok(None),
            Ok(token) => {
                let body = self.parser.get_body(&token);
                let mut buf = BytesMut::with_capacity(body.0.len() + body.1.len());
                buf.extend_from_slice(body.0);
                buf.extend_from_slice(body.1);
                self.parser.consume(token.into());
                Ok(Some(Decoded::Frame(buf.freeze())))
            }
            Err(Error::Junk { token, kind }) => {
                self.parser.consume(token);
                Ok(Some(Decoded::Junk(kind)))
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decoded {
    Frame(Bytes),
    Junk(JunkKind),
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use super::*;
    use proptest::collection::vec;
    use proptest::prelude::*;
    use tokio_util::codec::{Decoder as _, Encoder as _};

    proptest! {
        #[test]
        fn test(
            first in vec(0x00u8..0xffu8, 1..1024),
            junk in vec(0x00u8..0xffu8, 1..1024),
            last in vec(0x00u8..0xffu8, 1..1024)
        ) {
            prop_assume!(!junk.windows(2).any(|s| s == STX));
            let mut buf = BytesMut::new();
            let mut encoder = Encoder::new();
            encoder.encode(first.as_slice(), &mut buf).unwrap();
            buf.extend_from_slice(&junk);
            encoder.encode(last.as_slice(), &mut buf).unwrap();
            let mut decoder = Decoder::new(VecDeque::with_capacity(4096));
            assert_eq!(decoder.decode(&mut buf).unwrap().unwrap(), Decoded::Frame(first.into()));
            assert_eq!(decoder.decode(&mut buf).unwrap().unwrap(), Decoded::Junk(JunkKind::InvalidStx));
            assert_eq!(decoder.decode(&mut buf).unwrap().unwrap(), Decoded::Frame(last.into()));
            assert_eq!(decoder.decode(&mut buf).unwrap(), None);
        }
    }
}
