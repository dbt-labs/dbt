use std::time::SystemTime;

pub fn current_time_micros() -> u128 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("SystemTime before UNIX EPOCH!")
        .as_micros()
}

pub fn time_micros(time: SystemTime) -> u128 {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .expect("SystemTime before UNIX EPOCH!")
        .as_micros()
}
