#![allow(dead_code)]

const KCAL_PER_KM: f32 = 60.0; // rough estimate

/// Accumulates cumulative walking stats across device counter resets.
///
/// The device resets its counters when a new session starts on the device,
/// so we detect backward jumps and treat them as wraps.
#[derive(Debug, Clone, Default)]
pub struct Stats {
    pub total_distance_km: f32,
    pub total_steps: u32,
    pub active_seconds: u32,
    pub current_speed_kmh: f32,
    pub calories: f32,

    last_dev_dist: u32,
    last_dev_steps: u32,
}

impl Stats {
    pub fn new() -> Self {
        Self::default()
    }

    /// Called each time a status packet arrives.
    ///
    /// - `dev_dist_raw`: device-reported distance (raw units, ÷100 = km)
    /// - `dev_steps`: device-reported step count
    /// - `speed_raw`: device-reported speed (÷10 = km/h)
    pub fn update(&mut self, dev_dist_raw: u32, dev_steps: u32, speed_raw: u8) {
        // Distance delta — handle device counter reset (backward jump)
        // saturating_sub gives 0 on counter reset/wrap — exactly what we want
        let dist_delta = dev_dist_raw.saturating_sub(self.last_dev_dist);
        self.last_dev_dist = dev_dist_raw;
        self.total_distance_km += dist_delta as f32 / 100.0;

        let steps_delta = dev_steps.saturating_sub(self.last_dev_steps);
        self.last_dev_steps = dev_steps;
        self.total_steps += steps_delta;

        self.current_speed_kmh = speed_raw as f32 / 10.0;
        self.calories = self.total_distance_km * KCAL_PER_KM;
    }

    /// Tick one second of active walking time.
    pub fn tick_second(&mut self) {
        self.active_seconds += 1;
    }

    /// Reset all accumulators (new session).
    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_first_update_no_distance_spike() {
        let mut s = Stats::new();
        // First packet: device reports dist=50 (0.50 km).
        // Without tracking last_dev, a naive impl would add 0.50 km.
        // With correct tracking: delta = 50 - 0 = 50 → 0.50 km. That IS correct.
        s.update(50, 100, 30);
        assert!((s.total_distance_km - 0.50).abs() < 0.001);
        assert_eq!(s.total_steps, 100);
    }

    #[test]
    fn test_cumulative_distance_two_updates() {
        let mut s = Stats::new();
        s.update(50, 100, 30);
        s.update(80, 160, 30); // delta = 30 → +0.30 km
        assert!((s.total_distance_km - 0.80).abs() < 0.001);
        assert_eq!(s.total_steps, 160);
    }

    #[test]
    fn test_device_counter_reset_does_not_subtract() {
        let mut s = Stats::new();
        s.update(500, 1000, 30); // 5.00 km so far
        s.update(10, 20, 30);    // device reset → delta treated as 0
        assert!((s.total_distance_km - 5.00).abs() < 0.001);
        assert_eq!(s.total_steps, 1000);
    }

    #[test]
    fn test_stats_reset_clears_everything() {
        let mut s = Stats::new();
        s.update(200, 400, 30);
        s.tick_second();
        s.tick_second();
        s.reset();
        assert_eq!(s.total_distance_km, 0.0);
        assert_eq!(s.total_steps, 0);
        assert_eq!(s.active_seconds, 0);
    }

    #[test]
    fn test_tick_second_increments() {
        let mut s = Stats::new();
        s.tick_second();
        s.tick_second();
        s.tick_second();
        assert_eq!(s.active_seconds, 3);
    }

    #[test]
    fn test_speed_decoded_correctly() {
        let mut s = Stats::new();
        s.update(0, 0, 30);
        assert!((s.current_speed_kmh - 3.0).abs() < f32::EPSILON);
        s.update(0, 0, 60);
        assert!((s.current_speed_kmh - 6.0).abs() < f32::EPSILON);
    }

    #[test]
    fn test_calories_non_zero_after_distance() {
        let mut s = Stats::new();
        s.update(100, 200, 30); // 1.00 km
        assert!(s.calories > 0.0);
    }
}
