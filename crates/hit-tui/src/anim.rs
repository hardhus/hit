//! Small, input-never-waits animations.
//!
//! State changes happen immediately; an animation only interpolates how the
//! change is *drawn* for a few hundred milliseconds. With animations off the
//! end state is drawn right away.

use std::time::{Duration, Instant};

/// A value moving from `from` to `to`.
#[derive(Debug, Clone, Copy)]
pub struct Tween {
    from: f32,
    to: f32,
    start: Instant,
    duration: Duration,
}

impl Tween {
    pub fn settled(value: f32) -> Self {
        Self {
            from: value,
            to: value,
            start: Instant::now(),
            duration: Duration::ZERO,
        }
    }

    /// Starts moving from the current value to `to`.
    pub fn retarget(&mut self, to: f32, duration: Duration) {
        self.from = self.value();
        self.to = to;
        self.start = Instant::now();
        self.duration = duration;
    }

    pub fn target(&self) -> f32 {
        self.to
    }

    fn progress(&self) -> f32 {
        if self.duration.is_zero() {
            return 1.0;
        }
        (self.start.elapsed().as_secs_f32() / self.duration.as_secs_f32()).clamp(0.0, 1.0)
    }

    /// Eased (cubic ease-out) current value.
    pub fn value(&self) -> f32 {
        let t = ease_out(self.progress());
        self.from + (self.to - self.from) * t
    }

    pub fn is_running(&self) -> bool {
        self.progress() < 1.0
    }
}

/// Cubic ease-out: fast start, gentle landing.
pub fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t).powi(3)
}

/// A moment something happened, for "since" based effects (staggered rows,
/// pop-ins).
#[derive(Debug, Clone, Copy)]
pub struct Moment(Instant);

impl Moment {
    pub fn now() -> Self {
        Self(Instant::now())
    }

    /// 0..1 progress of an effect lasting `duration`, starting `delay`
    /// after this moment.
    pub fn progress(&self, delay: Duration, duration: Duration) -> f32 {
        let elapsed = self.0.elapsed().saturating_sub(delay);
        if duration.is_zero() {
            return 1.0;
        }
        (elapsed.as_secs_f32() / duration.as_secs_f32()).clamp(0.0, 1.0)
    }

    pub fn within(&self, duration: Duration) -> bool {
        self.0.elapsed() < duration
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn easing_endpoints() {
        assert_eq!(ease_out(0.0), 0.0);
        assert_eq!(ease_out(1.0), 1.0);
        assert!(ease_out(0.5) > 0.5, "ease-out is ahead of linear");
    }

    #[test]
    fn zero_duration_is_instant() {
        let mut tween = Tween::settled(0.0);
        tween.retarget(3.0, Duration::ZERO);
        assert_eq!(tween.value(), 3.0);
        assert!(!tween.is_running());
    }
}
