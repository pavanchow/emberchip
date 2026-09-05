//! A flat byte-addressable RAM. Task stacks live here as plain data, which is
//! how a real kernel treats them: a stack is just a region of memory a task
//! reads and writes.

#[derive(Clone, Debug)]
pub struct Memory {
    bytes: Vec<u8>,
}

impl Memory {
    pub fn new(size: usize) -> Self {
        Self {
            bytes: vec![0; size],
        }
    }

    pub fn size(&self) -> usize {
        self.bytes.len()
    }

    pub fn read(&self, addr: usize) -> Option<u8> {
        self.bytes.get(addr).copied()
    }

    pub fn write(&mut self, addr: usize, value: u8) -> bool {
        match self.bytes.get_mut(addr) {
            Some(slot) => {
                *slot = value;
                true
            }
            None => false,
        }
    }

    pub fn read_word(&self, addr: usize) -> Option<u32> {
        if addr + 4 > self.bytes.len() {
            return None;
        }
        let b = &self.bytes[addr..addr + 4];
        Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    pub fn write_word(&mut self, addr: usize, value: u32) -> bool {
        if addr + 4 > self.bytes.len() {
            return false;
        }
        self.bytes[addr..addr + 4].copy_from_slice(&value.to_le_bytes());
        true
    }
}

#[cfg(test)]
mod tests {
    use super::Memory;

    #[test]
    fn byte_round_trip() {
        let mut m = Memory::new(64);
        assert!(m.write(10, 0xAB));
        assert_eq!(m.read(10), Some(0xAB));
    }

    #[test]
    fn word_round_trip() {
        let mut m = Memory::new(64);
        assert!(m.write_word(8, 0xDEAD_BEEF));
        assert_eq!(m.read_word(8), Some(0xDEAD_BEEF));
    }

    #[test]
    fn out_of_bounds_is_none() {
        let mut m = Memory::new(4);
        assert_eq!(m.read(99), None);
        assert!(!m.write(99, 1));
        assert!(!m.write_word(2, 1));
    }
}
