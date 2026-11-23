use std::time::{Duration, Instant};

/// Deadlines are measured from the first pending change, so later ticks cannot postpone it.
#[derive(Default)]
pub(super) struct Timing {
    pub due: [Option<Instant>; 3],
    retrying: [bool; 3],
}

pub(super) const EDIT: Duration = Duration::from_millis(300);
pub(super) const POSITION: Duration = Duration::from_secs(15);
pub(super) const LISTENING: Duration = Duration::from_secs(20);

impl Timing {
    pub fn mark(&mut self, file: usize, now: Instant, delay: Duration) {
        if self.retrying[file] {
            return;
        }
        let at = now + delay;
        let due = self.due[file].get_or_insert(at);
        *due = (*due).min(at);
    }
    pub fn failed(&mut self, file: usize, now: Instant) {
        self.due[file] = Some(now + Duration::from_secs(10));
        self.retrying[file] = true;
    }
    pub fn remaining(&self, now: Instant) -> Option<Duration> {
        self.due
            .iter()
            .flatten()
            .min()
            .map(|at| at.saturating_duration_since(now))
    }
    pub fn take(&mut self, now: Instant) -> [bool; 3] {
        std::array::from_fn(|file| {
            let ready = self.due[file].is_some_and(|at| at <= now);
            if ready {
                self.due[file] = None;
                self.retrying[file] = false;
            }
            ready
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uninterrupted_play_writes_position_four_times_and_listening_three_times_a_minute() {
        let start = Instant::now();
        let mut clock = Timing::default();
        let mut writes = [vec![], vec![]];
        for sec in 0..=60 {
            let now = start + Duration::from_secs(sec);
            for (file, delay) in [POSITION, LISTENING].into_iter().enumerate() {
                clock.mark(file, now, delay);
            }
            let due = clock.take(now);
            for file in 0..2 {
                if due[file] {
                    writes[file].push(sec);
                    clock.mark(file, now, [POSITION, LISTENING][file]);
                }
            }
        }
        assert_eq!(writes, [vec![15, 30, 45, 60], vec![20, 40, 60]]);
    }

    #[test]
    fn pause_or_song_change_brings_the_position_save_forward() {
        let start = Instant::now();
        let mut clock = Timing::default();
        clock.mark(0, start, POSITION);
        clock.mark(1, start, LISTENING);
        clock.mark(0, start + Duration::from_secs(3), EDIT);
        assert_eq!(
            clock.take(start + Duration::from_secs(4)),
            [true, false, false]
        );
    }

    #[test]
    fn a_state_retry_does_not_delay_a_cache_edit() {
        let now = Instant::now();
        let mut clock = Timing::default();
        clock.failed(0, now);
        clock.mark(0, now, EDIT);
        clock.mark(2, now, EDIT);
        assert_eq!(clock.take(now + EDIT), [false, false, true]);
    }

    #[test]
    fn a_failed_batch_waits_ten_seconds_even_with_new_edits_and_playback_ticks() {
        for delay in [EDIT, POSITION] {
            let start = Instant::now();
            let mut clock = Timing::default();
            clock.failed(0, start);
            clock.mark(0, start, delay);
            assert_eq!(clock.remaining(start), Some(Duration::from_secs(10)));
            assert_eq!(clock.take(start + Duration::from_secs(9)), [false; 3]);
            assert_eq!(
                clock.take(start + Duration::from_secs(10)),
                [true, false, false]
            );
        }
    }
}
