//! Bink reads its bitstreams least significant bit first.

pub struct Bits<'a> {
    data: &'a [u8],
    pos: usize,
    end: usize,
}

impl<'a> Bits<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Bits { data, pos: 0, end: data.len() * 8 }
    }

    /// Bits read so far.
    pub fn pos(&self) -> usize {
        self.pos
    }

    pub fn left(&self) -> isize {
        self.end as isize - self.pos as isize
    }

    /// The next `n` (up to 32) bits without taking them.
    pub fn peek(&self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        let byte = self.pos >> 3;
        let mut w = 0u64;
        for i in 0..8 {
            w |= (*self.data.get(byte + i).unwrap_or(&0) as u64) << (8 * i);
        }
        ((w >> (self.pos & 7)) & ((1u64 << n) - 1)) as u32
    }

    pub fn read(&mut self, n: u32) -> u32 {
        let v = self.peek(n);
        self.pos += n as usize;
        v
    }

    pub fn bit(&mut self) -> bool {
        self.read(1) != 0
    }

    pub fn skip(&mut self, n: usize) {
        self.pos += n;
    }

    /// To the next 32-bit boundary.
    pub fn align32(&mut self) {
        let r = self.pos & 31;
        if r != 0 {
            self.pos += 32 - r;
        }
    }
}
