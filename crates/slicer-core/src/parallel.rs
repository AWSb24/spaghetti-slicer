//! Running the same job for many layers on all cores.

use std::sync::atomic::{AtomicUsize, Ordering};

/// Calls `job` for every index below `count` on all available cores and
/// returns the results in index order.
pub fn parallel_map<T: Send>(count: usize, job: impl Fn(usize) -> T + Sync) -> Vec<T> {
    let threads = std::thread::available_parallelism().map_or(1, usize::from);
    let next = AtomicUsize::new(0);
    let mut results: Vec<(usize, T)> = std::thread::scope(|scope| {
        let workers: Vec<_> = (0..threads.min(count))
            .map(|_| {
                scope.spawn(|| {
                    let mut done = Vec::new();
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        if index >= count {
                            break done;
                        }
                        done.push((index, job(index)));
                    }
                })
            })
            .collect();
        workers
            .into_iter()
            .flat_map(|worker| {
                worker
                    .join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
            })
            .collect()
    });
    results.sort_by_key(|(index, _)| *index);
    results.into_iter().map(|(_, result)| result).collect()
}

#[cfg(test)]
mod tests {
    use super::parallel_map;

    #[test]
    fn parallel_map_keeps_the_order() {
        let squares = parallel_map(1000, |i| i * i);
        assert!(squares.iter().enumerate().all(|(i, &s)| s == i * i));
        assert_eq!(squares.len(), 1000);
        assert!(parallel_map(0, |i| i).is_empty());
    }
}
