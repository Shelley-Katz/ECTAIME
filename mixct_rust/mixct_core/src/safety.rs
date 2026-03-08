pub fn clamp_db(value: f32, min_db: f32, max_db: f32) -> f32 {
    value.clamp(min_db, max_db)
}

pub fn enforce_slew(previous: f32, target: f32, max_delta_per_step: f32) -> f32 {
    let delta = target - previous;
    if delta.abs() <= max_delta_per_step {
        target
    } else {
        previous + delta.signum() * max_delta_per_step
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clamp_db_works() {
        assert_eq!(clamp_db(10.0, -6.0, 6.0), 6.0);
        assert_eq!(clamp_db(-9.0, -6.0, 6.0), -6.0);
    }

    #[test]
    fn slew_limits_delta() {
        let v = enforce_slew(0.0, 5.0, 1.0);
        assert_eq!(v, 1.0);
    }
}
