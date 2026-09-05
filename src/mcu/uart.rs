//! A memory-mapped UART, output only. Writes are appended to a transmit buffer
//! the host can read back, standing in for bytes leaving the chip on a wire.

#[derive(Clone, Debug, Default)]
pub struct Uart {
    tx: String,
    bytes_out: u64,
}

impl Uart {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn write(&mut self, text: &str) {
        self.bytes_out += text.len() as u64;
        self.tx.push_str(text);
    }

    pub fn writeln(&mut self, text: &str) {
        self.write(text);
        self.write("\n");
    }

    pub fn contents(&self) -> &str {
        &self.tx
    }

    pub fn bytes_out(&self) -> u64 {
        self.bytes_out
    }
}

#[cfg(test)]
mod tests {
    use super::Uart;

    #[test]
    fn appends_output() {
        let mut u = Uart::new();
        u.write("ab");
        u.writeln("c");
        assert_eq!(u.contents(), "abc\n");
        assert_eq!(u.bytes_out(), 4);
    }
}
