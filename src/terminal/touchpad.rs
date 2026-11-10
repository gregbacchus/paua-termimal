//! Turn timestamped touchpad deltas into a speed-dependent scroll gain.

const BOOST_START: f32 = 700.0; // input pixels per second
const BOOST_FULL: f32 = 2200.0;
const SMOOTHING_MS: f32 = 20.0;
const IDLE_MS: u32 = 100;

#[derive(Default)]
pub(super) struct Swipe {
    last_ms: Option<u32>,
    speed: f32,
}

impl Swipe {
    /// Slow motion keeps a gain of 1. Fast motion travels up to twice as far for the same delta.
    pub(super) fn gain(&mut self, delta: f32, time_ms: u32) -> f32 {
        let elapsed = self.last_ms.replace(time_ms).map(|last| time_ms.wrapping_sub(last));
        if self.speed.signum() != delta.signum() {
            self.speed = 0.0;
        }
        match elapsed {
            Some(0) => {} // Events can share a timestamp.
            Some(ms) if ms <= IDLE_MS => {
                let ms = ms as f32;
                let sample = delta * 1000.0 / ms;
                let weight = 1.0 - (-ms / SMOOTHING_MS).exp();
                self.speed += (sample - self.speed) * weight;
            }
            _ => self.speed = 0.0,
        }
        1.0 + ((self.speed.abs() - BOOST_START) / (BOOST_FULL - BOOST_START)).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::Swipe;

    #[test]
    fn faster_swipes_travel_farther_with_the_same_input_distance() {
        let swipe_distance = |interval: u32| {
            let mut swipe = Swipe::default();
            (0..8).map(|i| 12.0 * swipe.gain(12.0, i * interval)).sum::<f32>()
        };
        assert_eq!(swipe_distance(30), 96.0);
        assert!(swipe_distance(6) > 125.0);
    }

    #[test]
    fn a_pause_or_reversal_drops_the_previous_boost() {
        let mut swipe = Swipe::default();
        for time in [100, 108, 116, 124] {
            swipe.gain(-25.0, time);
        }
        assert!(swipe.gain(-25.0, 132) > 1.0);
        assert_eq!(swipe.gain(2.0, 140), 1.0);
        for time in [148, 156, 164, 172] {
            swipe.gain(25.0, time);
        }
        assert!(swipe.gain(25.0, 180) > 1.0);
        assert_eq!(swipe.gain(2.0, 350), 1.0);
    }
}
