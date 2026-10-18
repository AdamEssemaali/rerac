//! Bevy's clocks as rc-engine reads them. Every frame advances by the frame pace's `n` fixed steps of 1/60 s
//! (rc-engine `frame_pace` sets Bevy's `TimeUpdateStrategy::FixedTimesteps(n)`, under which `Time<Real>` and
//! `Time<Virtual>` advance by exactly n/60 s, not by the wall clock), and the fixed loop runs n times.

use std::time::Duration;

/// Bevy's `Time<Real>` / `Time<Virtual>` / `Time<Fixed>` / `Time` (the default clock: virtual in the frame, fixed
/// inside the fixed loop), one struct per kind.
#[derive(Clone, Copy, Debug, Default)]
pub struct Clock {
    pub delta: Duration,
    pub elapsed: Duration,
}

impl Clock {
    pub fn delta(&self) -> Duration { self.delta }
    pub fn delta_secs(&self) -> f32 { self.delta.as_secs_f32() }
    pub fn delta_secs_f64(&self) -> f64 { self.delta.as_secs_f64() }
    pub fn elapsed(&self) -> Duration { self.elapsed }
    pub fn elapsed_secs(&self) -> f32 { self.elapsed.as_secs_f32() }
    pub fn elapsed_secs_f64(&self) -> f64 { self.elapsed.as_secs_f64() }

    pub fn advance(&mut self, d: Duration) {
        self.delta = d;
        self.elapsed += d;
    }
}

/// The 60 Hz game tick.
pub const STEP: Duration = Duration::from_nanos(1_000_000_000 / 60);

/// The clocks as a resource.
#[derive(Clone, Copy, Debug)]
pub struct Time {
    pub real: Clock,
    pub virt: Clock,
    pub fixed: Clock,
    /// Bevy's `Time` (generic default): `virt` during the frame, `fixed` inside the fixed loop.
    pub default: Clock,
    pub fixed_timestep: Duration,
    /// Fixed steps this frame (`TimeUpdateStrategy::FixedTimesteps(n)`).
    pub steps: u32,
    /// The clocks have been updated once (Bevy's first update has no previous instant: zero delta, no fixed step).
    started: bool,
}

impl Default for Time {
    fn default() -> Self {
        Time { real: Clock::default(), virt: Clock::default(), fixed: Clock::default(), default: Clock::default(), fixed_timestep: STEP, steps: 1, started: false }
    }
}

impl Time {
    /// Bevy's `time_system` under `FixedTimesteps(n)`. The first update only records its instant (`Time<Real>`'s
    /// `update_with_instant` without a last update): every clock's delta is zero and the fixed loop does not run.
    pub fn begin_frame(&mut self, n: u32) {
        let n = if self.started { n } else { 0 };
        self.started = true;
        self.steps = n;
        let d = STEP * n;
        self.real.advance(d);
        self.virt.advance(d);
        self.default = self.virt;
    }

    /// Entering one fixed step.
    pub fn begin_fixed_step(&mut self) {
        self.fixed.advance(self.fixed_timestep);
        self.default = self.fixed;
    }

    pub fn end_fixed_loop(&mut self) { self.default = self.virt; }

    pub fn timestep(&self) -> Duration { self.fixed_timestep }
}

/// Bevy's `TimeUpdateStrategy::FixedTimesteps(n)`: the fixed steps of the coming frame (set by the frame pace).
#[derive(Clone, Copy, Debug)]
pub struct Steps(pub u32);

impl Default for Steps {
    fn default() -> Self { Steps(1) }
}
