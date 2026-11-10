//! How both ends of a connection notice the other has gone, without waiting for the
//! transport to. A laptop with its lid closed sends nothing, not even a close, so a
//! connection to it looks fine until QUIC gives up half a minute later, and what was sent
//! to it meanwhile would all arrive when it wakes.
//!
//! The client pings on a timer, and the daemon's websocket answers every ping. An end that
//! hears nothing for too long ends the connection, and what was waiting on it fails. The
//! daemon lets go first, so a command can't run after its client stopped waiting for it.

use std::time::{Duration, Instant, SystemTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Liveness {
    /// How often the client pings.
    pub ping_every: Duration,
    /// How long the client hears nothing before it shows the device as not responding.
    pub stalled_after: Duration,
    /// How long the client hears nothing before it ends the connection.
    pub silence_limit: Duration,
}

impl Default for Liveness {
    fn default() -> Self {
        Self {
            ping_every: Duration::from_secs(2),
            stalled_after: Duration::from_secs(5),
            // QUIC resends lost packets at growing gaps, so a few seconds without a network
            // can be twice that without a byte. This rides out such a gap.
            silence_limit: Duration::from_secs(10),
        }
    }
}

impl Liveness {
    /// How long the daemon hears nothing from a client before it ends the connection. One
    /// ping less than the client's limit, so the daemon is always the first to let go.
    pub fn daemon_limit(&self) -> Duration {
        self.silence_limit.saturating_sub(self.ping_every)
    }
}

/// Measures how long this machine was asleep, which timers never see: `Instant` stops while
/// the machine sleeps, and the wall clock doesn't. A clock that's set forward looks the
/// same, and costs one reconnect.
#[derive(Debug)]
pub struct SleepWatch {
    wall: SystemTime,
    awake: Instant,
}

impl SleepWatch {
    pub fn start() -> Self {
        Self {
            wall: SystemTime::now(),
            awake: Instant::now(),
        }
    }

    /// How long the machine slept since this was last asked, or since the start.
    pub fn slept(&mut self) -> Duration {
        let now = Self::start();
        let wall = now.wall.duration_since(self.wall).unwrap_or_default();
        let awake = now.awake.duration_since(self.awake);
        *self = now;
        asleep(wall, awake)
    }
}

/// A wall clock that was set back reads as no time at all, which is no sleep.
fn asleep(wall: Duration, awake: Duration) -> Duration {
    wall.saturating_sub(awake)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_daemon_gives_up_before_the_client_does() {
        let liveness = Liveness::default();
        assert!(liveness.daemon_limit() < liveness.silence_limit);
        assert!(liveness.ping_every < liveness.stalled_after);
        assert!(liveness.stalled_after < liveness.daemon_limit());
    }

    #[test]
    fn sleep_is_the_time_only_the_wall_clock_counted() {
        let secs = Duration::from_secs;
        assert_eq!(asleep(secs(2), secs(2)), secs(0));
        assert_eq!(asleep(secs(62), secs(2)), secs(60));
        // The clock was set back while the machine stayed awake.
        assert_eq!(asleep(secs(0), secs(2)), secs(0));
    }

    #[test]
    fn a_machine_that_stayed_awake_did_not_sleep() {
        let mut watch = SleepWatch::start();
        assert!(watch.slept() < Duration::from_secs(1));
    }
}
