#![allow(clippy::unwrap_used, clippy::expect_used)]

pub mod monitors;
pub mod window;

use std::sync::Mutex;
use std::time::{Duration, Instant};

pub static CAMERA_TEST_LOCK: Mutex<()> = Mutex::new(());
pub const FIRST: [u8; 3] = [220, 40, 150];
pub const SECOND: [u8; 3] = [30, 200, 90];

pub fn wait_for<T>(mut read: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(value) = read() {
            return value;
        }
        assert!(
            Instant::now() < deadline,
            "camera did not capture the expected screen"
        );
        std::thread::sleep(Duration::from_millis(25));
    }
}

pub fn color_matches(actual: [u8; 3], expected: [u8; 3]) -> bool {
    actual
        .into_iter()
        .zip(expected)
        .all(|(a, b)| a.abs_diff(b) <= 20)
}

pub fn pixel(image: &image::RgbImage, x: i32, y: i32) -> [u8; 3] {
    image
        .get_pixel(u32::try_from(x).unwrap(), u32::try_from(y).unwrap())
        .0
}
