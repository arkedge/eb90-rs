use core::marker::PhantomData;

use crate::{crc, CRC_SIZE, ETX, FOOTER_SIZE, HEADER_SIZE, STX};

pub trait Buffer {
    fn capacity(&self) -> usize;
    fn len(&self) -> usize;
    fn data(&self) -> (&[u8], &[u8]);
    fn consume(&mut self, len: usize);
    fn write(&mut self, input: &[u8]);

    fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl<const N: usize> Buffer for heapless::Deque<u8, N> {
    fn capacity(&self) -> usize {
        heapless::Deque::capacity(self)
    }

    fn len(&self) -> usize {
        heapless::Deque::len(self)
    }

    fn data(&self) -> (&[u8], &[u8]) {
        heapless::Deque::as_slices(self)
    }

    fn consume(&mut self, size: usize) {
        assert!(heapless::Deque::len(self) >= size);
        for _ in 0..size {
            unsafe {
                heapless::Deque::pop_front_unchecked(self);
            }
        }
    }

    fn write(&mut self, input: &[u8]) {
        assert!(heapless::Deque::capacity(self) - heapless::Deque::len(self) >= input.len());
        for &b in input {
            unsafe { heapless::Deque::push_back_unchecked(self, b) }
        }
    }
}

#[cfg(any(test, feature = "alloc"))]
mod alloc_support {
    extern crate alloc;
    use super::*;
    use alloc::collections::VecDeque;

    impl Buffer for VecDeque<u8> {
        fn capacity(&self) -> usize {
            VecDeque::capacity(self)
        }

        fn len(&self) -> usize {
            VecDeque::len(self)
        }

        fn data(&self) -> (&[u8], &[u8]) {
            VecDeque::as_slices(self)
        }

        fn consume(&mut self, len: usize) {
            VecDeque::drain(self, ..len);
        }

        fn write(&mut self, input: &[u8]) {
            assert!(VecDeque::capacity(self) - VecDeque::len(self) >= input.len());
            VecDeque::extend(self, input.iter());
        }
    }
}

#[cfg(any(test, feature = "alloc"))]
#[doc(inline)]
pub use alloc_support::*;

#[inline]
fn slices_start<'a>((a, b): (&'a [u8], &'a [u8]), start: usize) -> (&'a [u8], &'a [u8]) {
    debug_assert!(start < a.len() + b.len());
    if start < a.len() {
        (&a[start..], b)
    } else {
        (&[], &b[start - a.len()..])
    }
}

#[inline]
fn slices_endx<'a>((a, b): (&'a [u8], &'a [u8]), endx: usize) -> (&'a [u8], &'a [u8]) {
    debug_assert!(endx <= a.len() + b.len());
    if endx <= a.len() {
        (&a[..endx], &[])
    } else {
        (&[], &b[..endx - a.len()])
    }
}

#[inline]
fn slices_read_u16((a, b): (&[u8], &[u8]), index: usize) -> u16 {
    use core::cmp::Ordering;
    let s = index;
    let t = index + 1;
    match a.len().cmp(&t) {
        Ordering::Greater => u16::from_be_bytes([a[s], a[t]]),
        Ordering::Less => u16::from_be_bytes([b[s - a.len()], b[t - a.len()]]),
        Ordering::Equal => u16::from_be_bytes([a[s], b[0]]),
    }
}

pub struct FrameToken<'a> {
    body_size: usize,
    phantom: PhantomData<&'a ()>,
}

impl<'a> FrameToken<'a> {
    #[doc(hidden)]
    pub fn forge<'b>(self) -> FrameToken<'b> {
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
    Junk { token: ConsumeToken, kind: JunkKind },
}

pub enum JunkKind {
    InvalidStx,
    InvalidLength,
    InvalidCrc,
    InvalidEtx,
}

#[derive(Default)]
pub struct Parser<B> {
    buf: B,
}

impl<B> Parser<B>
where
    B: Buffer,
{
    fn find_stx(&self) -> Option<usize> {
        let (buf_a, buf_b) = self.buf.data();
        if let Some(p) = buf_a.windows(STX.len()).position(|win| win == STX) {
            return Some(p);
        }
        if buf_a.len() & 1 == 1 && !buf_b.is_empty() && [buf_a[buf_a.len() - 1], buf_b[0]] == STX {
            return Some(buf_a.len() - 1);
        }
        if let Some(p) = buf_b.windows(STX.len()).position(|win| win == STX) {
            return Some(p + buf_a.len());
        }
        None
    }

    pub fn with_buffer(buf: B) -> Self {
        Self { buf }
    }

    #[inline]
    pub fn fill(&mut self, input: &[u8]) -> usize {
        let copy_len = input.len().min(self.buf.capacity() - self.buf.len());
        self.buf.write(&input[..copy_len]);
        copy_len
    }

    pub fn is_full(&self) -> bool {
        self.buf.len() == self.buf.capacity()
    }

    pub fn consume(&mut self, token: ConsumeToken) {
        let ConsumeToken { len } = token;
        self.buf.consume(len);
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
                    return Err(Error::Junk {
                        token: ConsumeToken { len: STX.len() },
                        kind: JunkKind::InvalidLength,
                    });
                }
                if self.buf.len() < frame_size {
                    return Err(Error::Incomplete);
                }
                let tail = slices_start(self.buf.data(), HEADER_SIZE);
                let footer = slices_start(tail, body_size);
                let body = slices_endx(tail, body_size);
                let expected_crc = slices_read_u16(footer, 0);
                let etx = slices_read_u16(footer, CRC_SIZE);
                if etx != u16::from_be_bytes(ETX) {
                    return Err(Error::Junk {
                        token: ConsumeToken { len: STX.len() },
                        kind: JunkKind::InvalidEtx,
                    });
                }
                let mut digest = crc::ALGO.digest();
                digest.update(body.0);
                digest.update(body.1);
                let actual_crc = digest.finalize();
                if expected_crc != actual_crc {
                    return Err(Error::Junk {
                        token: ConsumeToken { len: STX.len() },
                        kind: JunkKind::InvalidCrc,
                    });
                }
                Ok(FrameToken {
                    body_size,
                    phantom: PhantomData,
                })
            }
            Some(pos) => Err(Error::Junk {
                token: ConsumeToken { len: pos },
                kind: JunkKind::InvalidStx,
            }),
            None => Err(Error::Junk {
                token: ConsumeToken {
                    len: self.buf.len() - (STX.len() - 1),
                },
                kind: JunkKind::InvalidStx,
            }),
        }
    }

    pub fn get_body(&self, token: &FrameToken) -> (&[u8], &[u8]) {
        slices_endx(slices_start(self.buf.data(), HEADER_SIZE), token.body_size)
    }

    fn max_frame_size(&self) -> usize {
        self.buf.capacity()
    }

    fn body_size(&self) -> u16 {
        debug_assert!(self.buf.len() >= HEADER_SIZE);
        slices_read_u16(self.buf.data(), STX.len())
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

    fn make_contiguous((a, b): (&[u8], &[u8])) -> Vec<u8> {
        let mut v = Vec::with_capacity(a.len() + b.len());
        v.extend_from_slice(a);
        v.extend_from_slice(b);
        v
    }

    #[test]
    fn test_empty_input() {
        let mut rdr = Parser::with_buffer(heapless::Deque::<u8, 32>::new());
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
            let mut rdr = Parser::with_buffer(heapless::Deque::<u8, 12>::new());
            let last = segs.pop().unwrap();
            for seg in segs {
                assert_eq!(rdr.fill(seg), seg.len());
            }
            assert_eq!(rdr.fill(last), last.len());
            if let Ok(ft) = rdr.read() {
                assert_eq!(ft.body_size, 4);
                assert_eq!(make_contiguous(rdr.get_body(&ft)), DEADBEEF);
            } else {
                panic!();
            }
        }

        #[test]
        fn test_insufficient_buf(mut segs in chop(VALID_DEADBEEF_CASE)) {
            const BUF_SIZE: usize = VALID_DEADBEEF_CASE.len() - 1;
            let mut rdr = Parser::with_buffer(heapless::Deque::<u8, BUF_SIZE>::new());
            let last = segs.pop().unwrap();
            for seg in segs {
                assert_eq!(rdr.fill(seg), seg.len());
            }
            assert_ne!(rdr.fill(last), last.len());
            assert!(matches!(rdr.read(), Err(Error::Junk { .. })));
        }

        #[test]
        fn test_double_input(segs in chop(&VALID_DEADBEEF_CASE.iter().chain(VALID_HELLOWORLD_CASE.iter()).cloned().collect::<Vec<_>>())) {
            let mut rdr = Parser::with_buffer(heapless::Deque::<u8, 32>::new());
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
                                found.push(make_contiguous(body));
                                let t = ft.into();
                                rdr.consume(t);
                            },
                            Err(Error::Junk { token, .. }) => {
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
