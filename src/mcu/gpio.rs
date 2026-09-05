//! A memory-mapped GPIO port. Pins are boolean levels. Pin 0 is wired to the
//! on-board LED in the demo, so a task toggling pin 0 makes the LED blink.

pub const PIN_COUNT: usize = 8;
pub const LED_PIN: usize = 0;

#[derive(Clone, Debug)]
pub struct Gpio {
    pins: [bool; PIN_COUNT],
}

impl Default for Gpio {
    fn default() -> Self {
        Self::new()
    }
}

impl Gpio {
    pub fn new() -> Self {
        Self {
            pins: [false; PIN_COUNT],
        }
    }

    pub fn get(&self, pin: usize) -> Option<bool> {
        self.pins.get(pin).copied()
    }

    pub fn set(&mut self, pin: usize, level: bool) -> bool {
        match self.pins.get_mut(pin) {
            Some(slot) => {
                *slot = level;
                true
            }
            None => false,
        }
    }

    /// Flip a pin and return the new level. `None` if the pin does not exist.
    pub fn toggle(&mut self, pin: usize) -> Option<bool> {
        let slot = self.pins.get_mut(pin)?;
        *slot = !*slot;
        Some(*slot)
    }

    pub fn led(&self) -> bool {
        self.pins[LED_PIN]
    }
}

#[cfg(test)]
mod tests {
    use super::Gpio;

    #[test]
    fn set_and_get() {
        let mut g = Gpio::new();
        assert!(g.set(2, true));
        assert_eq!(g.get(2), Some(true));
    }

    #[test]
    fn toggle_flips() {
        let mut g = Gpio::new();
        assert_eq!(g.toggle(0), Some(true));
        assert_eq!(g.toggle(0), Some(false));
        assert!(!g.led());
    }

    #[test]
    fn bad_pin() {
        let mut g = Gpio::new();
        assert_eq!(g.get(99), None);
        assert!(!g.set(99, true));
        assert_eq!(g.toggle(99), None);
    }
}
