//! A minimal interrupt controller. Peripherals raise IRQ lines, the controller
//! latches them as pending, and the kernel drains the pending set each tick to
//! dispatch the matching ISR. This models the hardware path a real MCU takes
//! from a peripheral event to a software handler.

/// Interrupt sources on the emulated chip.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Irq {
    /// System tick timer wrapped.
    Timer,
    /// UART finished a transmission.
    Uart,
    /// A GPIO pin changed and was configured to interrupt.
    Gpio(usize),
}

#[derive(Clone, Debug, Default)]
pub struct InterruptController {
    pending: Vec<Irq>,
    serviced: u64,
}

impl InterruptController {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn raise(&mut self, irq: Irq) {
        if !self.pending.contains(&irq) {
            self.pending.push(irq);
        }
    }

    pub fn is_pending(&self, irq: Irq) -> bool {
        self.pending.contains(&irq)
    }

    /// Take the pending interrupts for servicing, highest priority first.
    /// Timer outranks UART outranks GPIO, matching a typical vector priority.
    pub fn take_pending(&mut self) -> Vec<Irq> {
        let mut out = std::mem::take(&mut self.pending);
        out.sort();
        self.serviced += out.len() as u64;
        out
    }

    pub fn serviced(&self) -> u64 {
        self.serviced
    }
}

#[cfg(test)]
mod tests {
    use super::{InterruptController, Irq};

    #[test]
    fn raise_and_drain() {
        let mut ic = InterruptController::new();
        ic.raise(Irq::Gpio(3));
        ic.raise(Irq::Timer);
        assert!(ic.is_pending(Irq::Timer));
        let drained = ic.take_pending();
        assert_eq!(drained, vec![Irq::Timer, Irq::Gpio(3)]);
        assert!(!ic.is_pending(Irq::Timer));
        assert_eq!(ic.serviced(), 2);
    }

    #[test]
    fn no_duplicate_pending() {
        let mut ic = InterruptController::new();
        ic.raise(Irq::Timer);
        ic.raise(Irq::Timer);
        assert_eq!(ic.take_pending().len(), 1);
    }
}
