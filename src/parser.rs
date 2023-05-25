use core::marker::PhantomData;
use core::ops::Deref;
use std::ops::DerefMut;

use crate::{crc, CRC_SIZE, ETX, FOOTER_SIZE, HEADER_SIZE, STX};

pub struct FrameToken<'a> {
    body_size: usize,
    phantom: PhantomData<&'a ()>,
}

impl<'a> FrameToken<'a> {
    pub(crate) fn forge<'b>(self) -> FrameToken<'b> {
        FrameToken {
            body_size: self.body_size,
            phantom: PhantomData,
        }
    }
}

pub struct ConsumeToken {
    len: usize,
}

impl<'a> From<FrameToken<'a>> for ConsumeToken {
    fn from(ft: FrameToken) -> Self {
        let len = ft.body_size + HEADER_SIZE + FOOTER_SIZE;
        ConsumeToken { len }
    }
}

pub enum Error {
    Incomplete,
    Junk(ConsumeToken),
}

pub trait Buffer: Deref<Target = [u8]> + DerefMut {
    #[allow(clippy::result_unit_err)]
    fn extend_from_slice(&mut self, other: &[u8]) -> Result<(), ()>;
    fn truncate(&mut self, len: usize);
    fn is_full(&self) -> bool;
    fn capacity(&self) -> usize;
}

impl<const N: usize> Buffer for heapless::Vec<u8, N> {
    #[inline]
    fn extend_from_slice(&mut self, other: &[u8]) -> Result<(), ()> {
        heapless::Vec::extend_from_slice(self, other)
    }

    #[inline]
    fn truncate(&mut self, len: usize) {
        heapless::Vec::truncate(self, len)
    }

    #[inline]
    fn is_full(&self) -> bool {
        heapless::Vec::is_full(self)
    }

    #[inline]
    fn capacity(&self) -> usize {
        heapless::Vec::capacity(self)
    }
}

#[cfg(any(test, feature = "alloc"))]
mod alloc_support {
    extern crate alloc;
    use super::Buffer;

    impl Buffer for alloc::vec::Vec<u8> {
        fn extend_from_slice(&mut self, other: &[u8]) -> Result<(), ()> {
            if Vec::len(self) + other.len() > Vec::capacity(self) {
                Err(())
            } else {
                Vec::extend_from_slice(self, other);
                Ok(())
            }
        }

        #[inline]
        fn truncate(&mut self, len: usize) {
            Vec::truncate(self, len);
        }

        #[inline]
        fn is_full(&self) -> bool {
            Vec::len(self) == Vec::capacity(self)
        }

        #[inline]
        fn capacity(&self) -> usize {
            Vec::capacity(self)
        }
    }
}

#[cfg(any(test, feature = "alloc"))]
#[doc(inline)]
pub use alloc_support::*;

#[derive(Default)]
pub struct Parser<B> {
    buf: B,
}

impl<B> Parser<B>
where
    B: Buffer,
{
    fn find_stx(&self) -> Option<usize> {
        self.buf.windows(STX.len()).position(|win| win == STX)
    }

    pub fn with_buffer(buf: B) -> Self {
        Self { buf }
    }

    #[inline]
    #[must_use]
    /// Attempts to write bytes from `input` into the internal buffer
    /// and returns the number of bytes are written.
    /// Note that this do not always the whole bytes of `input`,
    /// for example, when the internal buffer is insufficient.
    pub fn fill(&mut self, input: &[u8]) -> usize {
        let copy_len = input.len().min(self.buf.capacity() - self.buf.len());
        self.buf
            .extend_from_slice(&input[..copy_len])
            .expect("never panic");
        copy_len
    }

    pub fn is_full(&self) -> bool {
        self.buf.is_full()
    }

    pub fn consume(&mut self, token: ConsumeToken) {
        let ConsumeToken { len } = token;
        self.buf.copy_within(len.., 0);
        self.buf.truncate(self.buf.len() - len);
    }

    pub fn read(&self) -> Result<FrameToken, Error> {
        if self.buf.len() < HEADER_SIZE + FOOTER_SIZE {
            return Err(Error::Incomplete);
        }
        match self.find_stx() {
            Some(0) => {
                let body_size = self.body_size() as usize;
                let frame_size = body_size + HEADER_SIZE + FOOTER_SIZE;
                if frame_size > self.max_frame_size() {
                    return Err(Error::Junk(ConsumeToken { len: STX.len() }));
                }
                if self.buf.len() < frame_size {
                    return Err(Error::Incomplete);
                }
                let frame = &self.buf[..frame_size];
                let (_header, tail) = frame.split_at(HEADER_SIZE);
                let (body, footer) = tail.split_at(body_size);
                let (expected_crc, etx) = footer.split_at(CRC_SIZE);
                if etx != ETX {
                    return Err(Error::Junk(ConsumeToken { len: STX.len() }));
                }
                let actual_crc = crc::checksum(body);
                if expected_crc != actual_crc {
                    return Err(Error::Junk(ConsumeToken { len: STX.len() }));
                }
                Ok(FrameToken {
                    body_size,
                    phantom: PhantomData,
                })
            }
            Some(pos) => Err(Error::Junk(ConsumeToken { len: pos })),
            None => Err(Error::Junk(ConsumeToken {
                len: self.buf.len() - (STX.len() - 1),
            })),
        }
    }

    pub fn get_body(&self, token: &FrameToken) -> &[u8] {
        &self.buf[HEADER_SIZE..HEADER_SIZE + token.body_size]
    }

    fn max_frame_size(&self) -> usize {
        self.buf.capacity()
    }

    #[inline]
    fn body_size(&self) -> u16 {
        u16::from_be_bytes([self.buf[2], self.buf[3]])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const DEADBEEF: &[u8] = &[0xde, 0xad, 0xbe, 0xef];
    const HELLOWORLD: &[u8] = b"hello world";
    const VALID_DEADBEEF_CASE: &[u8] = &[
        0xeb, 0x90, 0x00, 0x04, 0xde, 0xad, 0xbe, 0xef, 0xe5, 0x9b, 0xc5, 0x79,
    ];
    const VALID_HELLOWORLD_CASE: &[u8] = &[
        0xeb, 0x90, 0x00, 0x0b, 0x68, 0x65, 0x6c, 0x6c, 0x6f, 0x20, 0x77, 0x6f, 0x72, 0x6c, 0x64,
        0x39, 0xc1, 0xc5, 0x79,
    ];

    #[test]
    fn test_empty_input() {
        let mut rdr = Parser::with_buffer(heapless::Vec::<_, 32>::new());
        let input = [];
        assert_eq!(rdr.fill(&input), 0);
    }

    /// バイト列を適当にぶつ切りにする proptest Strategy
    fn chop(bytes: &[u8]) -> impl Strategy<Value = Vec<&[u8]>> + '_ {
        let mut segs = vec![];
        for len in 1..=bytes.len() {
            for _ in 0..bytes.len() / len {
                segs.push(len);
            }
        }
        Just(segs).prop_shuffle().prop_map(move |segs| {
            let mut bytes = bytes;
            let mut parts = vec![];
            for seg in segs {
                if bytes.is_empty() {
                    break;
                }
                if seg > bytes.len() {
                    parts.push(bytes);
                    break;
                }
                parts.push(&bytes[..seg]);
                bytes = &bytes[seg..];
            }
            parts
        })
    }

    proptest! {
        #[test]
        fn test_reader(mut segs in chop(VALID_DEADBEEF_CASE)) {
            let mut rdr = Parser::with_buffer(heapless::Vec::<_, 12>::new());
            let last = segs.pop().unwrap();
            for seg in segs {
                assert_eq!(rdr.fill(seg), seg.len());
            }
            assert_eq!(rdr.fill(last), last.len());
            if let Ok(ft) = rdr.read() {
                assert_eq!(ft.body_size, 4);
                assert_eq!(rdr.get_body(&ft), DEADBEEF);
            } else {
                panic!();
            }
        }

        #[test]
        fn test_insufficient_buf(mut segs in chop(VALID_DEADBEEF_CASE)) {
            const BUF_SIZE: usize = VALID_DEADBEEF_CASE.len() - 1;
            let mut rdr = Parser::with_buffer(heapless::Vec::<_, BUF_SIZE>::new());
            let last = segs.pop().unwrap();
            for seg in segs {
                assert_eq!(rdr.fill(seg), seg.len());
            }
            assert_ne!(rdr.fill(last), last.len());
            assert!(matches!(rdr.read(), Err(Error::Junk(_))));
        }

        #[test]
        fn test_double_input(segs in chop(&VALID_DEADBEEF_CASE.iter().chain(VALID_HELLOWORLD_CASE.iter()).cloned().collect::<Vec<_>>())) {
            let mut rdr = Parser::with_buffer(heapless::Vec::<_, 32>::new());
            let mut iter = segs.into_iter();
            let mut found = vec![];
            for seg in &mut iter {
                let mut input = seg;
                while !input.is_empty() {
                    let filled = rdr.fill(seg);
                    input = &input[filled..];
                    loop {
                        match rdr.read() {
                            Ok(ft) => {
                                let body = rdr.get_body(&ft);
                                found.push(body.to_vec());
                                let t = ft.into();
                                rdr.consume(t);
                            },
                            Err(Error::Junk(token)) => {
                                rdr.consume(token);
                            },
                            Err(Error::Incomplete) => {
                                break;
                            },
                        }
                    }
                }
            }
            assert_eq!(found, vec![DEADBEEF, HELLOWORLD]);
        }
    }
}
