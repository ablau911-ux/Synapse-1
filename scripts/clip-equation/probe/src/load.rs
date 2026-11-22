//! Processor load for M7: one spinning thread per logical processor, at
//! normal priority, until dropped.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

pub struct BusyLoad {
    stop: Arc<AtomicBool>,
    threads: Vec<JoinHandle<()>>,
}

impl BusyLoad {
    pub fn start(threads: usize) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let threads = (0..threads)
            .map(|_| {
                let stop = stop.clone();
                std::thread::spawn(move || {
                    while !stop.load(Ordering::Relaxed) {
                        std::hint::spin_loop();
                    }
                })
            })
            .collect();
        Self { stop, threads }
    }
}

impl Drop for BusyLoad {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        for thread in self.threads.drain(..) {
            let _ = thread.join();
        }
    }
}
