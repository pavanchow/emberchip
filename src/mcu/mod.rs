//! The emulated microcontroller: memory, memory-mapped peripherals, an
//! interrupt controller, and a cycle counter. The kernel drives this hardware,
//! it does not reach around it.

pub mod gpio;
pub mod interrupt;
pub mod memory;
pub mod timer;
pub mod uart;

pub use gpio::{Gpio, LED_PIN, PIN_COUNT};
pub use interrupt::{InterruptController, Irq};
pub use memory::Memory;
pub use timer::Timer;
pub use uart::Uart;

/// Default RAM size for the emulated chip, in bytes.
pub const DEFAULT_RAM: usize = 64 * 1024;

#[derive(Clone, Debug)]
pub struct Mcu {
    pub memory: Memory,
    pub gpio: Gpio,
    pub uart: Uart,
    pub timer: Timer,
    pub nvic: InterruptController,
    cycles: u64,
}

impl Mcu {
    pub fn new(ram: usize, tick_reload: u64) -> Self {
        Self {
            memory: Memory::new(ram),
            gpio: Gpio::new(),
            uart: Uart::new(),
            timer: Timer::new(tick_reload),
            nvic: InterruptController::new(),
            cycles: 0,
        }
    }

    /// Advance the hardware one cycle. The timer may raise its interrupt, which
    /// is latched in the controller for the kernel to service.
    pub fn cycle(&mut self) {
        self.cycles += 1;
        if self.timer.clock() {
            self.nvic.raise(Irq::Timer);
        }
    }

    pub fn cycles(&self) -> u64 {
        self.cycles
    }
}

impl Default for Mcu {
    fn default() -> Self {
        Self::new(DEFAULT_RAM, 1)
    }
}

#[cfg(test)]
mod tests {
    use super::{Irq, Mcu};

    #[test]
    fn cycle_raises_tick_interrupt() {
        let mut mcu = Mcu::new(1024, 1);
        mcu.cycle();
        assert!(mcu.nvic.is_pending(Irq::Timer));
        assert_eq!(mcu.cycles(), 1);
    }
}
