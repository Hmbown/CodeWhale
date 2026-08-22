//! Terminal input ingestion and fairness controls for the TUI event loop.

use std::cell::Cell;
use std::io;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossterm::event::{self, Event};

const TERMINAL_INPUT_POLL_INTERVAL: Duration = Duration::from_millis(50);
const TERMINAL_INPUT_HEARTBEAT_INTERVAL: Duration = Duration::from_millis(500);
pub(super) const TERMINAL_INPUT_STALL_TIMEOUT: Duration = Duration::from_secs(5);
pub(super) const TERMINAL_INPUT_RECOVERY_COOLDOWN: Duration = Duration::from_secs(10);
const TERMINAL_INPUT_CHILD_PAUSE_TIMEOUT: Duration = Duration::from_millis(500);
const TERMINAL_INPUT_CHILD_PAUSE_POLL_INTERVAL: Duration = Duration::from_millis(5);
/// Upper bound on engine events processed before yielding to terminal input.
pub(super) const MAX_ENGINE_EVENTS_PER_DRAIN: usize = 16;
/// Wall-clock budget for one engine drain batch (#1830 / #2317 input fairness).
pub(super) const ENGINE_DRAIN_TIME_BUDGET: Duration = Duration::from_millis(8);

pub(super) enum TerminalInputMessage {
    Event(Event),
    Heartbeat,
    Error(io::Error),
}

pub(crate) struct TerminalInputPump {
    pub(super) rx: std::sync::mpsc::Receiver<TerminalInputMessage>,
    pub(super) stop: Arc<AtomicBool>,
    pub(super) paused: Arc<AtomicBool>,
    pub(super) paused_ack: Arc<AtomicBool>,
    pub(super) handle: Option<JoinHandle<()>>,
    pub(super) last_alive_at: Cell<Instant>,
}

pub(super) struct TerminalInputPumpParts {
    pub(super) rx: std::sync::mpsc::Receiver<TerminalInputMessage>,
    pub(super) stop: Arc<AtomicBool>,
    pub(super) paused: Arc<AtomicBool>,
    pub(super) paused_ack: Arc<AtomicBool>,
    pub(super) handle: JoinHandle<()>,
}

impl TerminalInputPump {
    pub(super) fn spawn() -> io::Result<Self> {
        let parts = Self::spawn_parts()?;
        Ok(Self {
            rx: parts.rx,
            stop: parts.stop,
            paused: parts.paused,
            paused_ack: parts.paused_ack,
            handle: Some(parts.handle),
            last_alive_at: Cell::new(Instant::now()),
        })
    }

    fn spawn_parts() -> io::Result<TerminalInputPumpParts> {
        let (tx, rx) = std::sync::mpsc::channel();
        let stop = Arc::new(AtomicBool::new(false));
        let paused = Arc::new(AtomicBool::new(false));
        let paused_ack = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let thread_paused = Arc::clone(&paused);
        let thread_paused_ack = Arc::clone(&paused_ack);
        let handle = thread::Builder::new()
            .name("codewhale-terminal-input".to_string())
            .spawn(move || {
                let mut last_heartbeat = Instant::now();
                while !thread_stop.load(Ordering::Acquire) {
                    if thread_paused.load(Ordering::Acquire) {
                        thread_paused_ack.store(true, Ordering::Release);
                        thread::sleep(TERMINAL_INPUT_CHILD_PAUSE_POLL_INTERVAL);
                        continue;
                    }
                    thread_paused_ack.store(false, Ordering::Release);
                    match event::poll(TERMINAL_INPUT_POLL_INTERVAL) {
                        Ok(true) => match event::read() {
                            Ok(event) => {
                                last_heartbeat = Instant::now();
                                if tx.send(TerminalInputMessage::Event(event)).is_err() {
                                    break;
                                }
                            }
                            Err(err) => {
                                let _ = tx.send(TerminalInputMessage::Error(err));
                                break;
                            }
                        },
                        Ok(false) => {
                            let now = Instant::now();
                            if now.duration_since(last_heartbeat)
                                >= TERMINAL_INPUT_HEARTBEAT_INTERVAL
                            {
                                last_heartbeat = now;
                                if tx.send(TerminalInputMessage::Heartbeat).is_err() {
                                    break;
                                }
                            }
                        }
                        Err(err) => {
                            let _ = tx.send(TerminalInputMessage::Error(err));
                            break;
                        }
                    }
                }
            })?;
        Ok(TerminalInputPumpParts {
            rx,
            stop,
            paused,
            paused_ack,
            handle,
        })
    }

    pub(super) fn recv_timeout(&self, timeout: Duration) -> io::Result<Option<Event>> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(remaining) {
                Ok(TerminalInputMessage::Event(event)) => {
                    self.mark_alive();
                    return Ok(Some(event));
                }
                Ok(TerminalInputMessage::Heartbeat) => {
                    self.mark_alive();
                    if remaining.is_zero() {
                        return Ok(None);
                    }
                }
                Ok(TerminalInputMessage::Error(err)) => {
                    self.mark_alive();
                    return Err(err);
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => return Ok(None),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "terminal input pump disconnected",
                    ));
                }
            }
        }
    }

    pub(super) fn try_recv(&self) -> io::Result<Option<Event>> {
        loop {
            match self.rx.try_recv() {
                Ok(TerminalInputMessage::Event(event)) => {
                    self.mark_alive();
                    return Ok(Some(event));
                }
                Ok(TerminalInputMessage::Heartbeat) => {
                    self.mark_alive();
                }
                Ok(TerminalInputMessage::Error(err)) => {
                    self.mark_alive();
                    return Err(err);
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => return Ok(None),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => return Ok(None),
            }
        }
    }

    pub(super) fn mark_alive(&self) {
        self.last_alive_at.set(Instant::now());
    }

    pub(super) fn stalled_for(&self, now: Instant) -> Duration {
        now.saturating_duration_since(self.last_alive_at.get())
    }

    pub(super) fn pause_for_child_terminal(&self) -> io::Result<()> {
        self.paused.store(true, Ordering::Release);
        if self.handle.is_none() {
            self.paused_ack.store(true, Ordering::Release);
            self.mark_alive();
            return Ok(());
        }

        let deadline = Instant::now() + TERMINAL_INPUT_CHILD_PAUSE_TIMEOUT;
        while !self.paused_ack.load(Ordering::Acquire) {
            if Instant::now() >= deadline {
                self.paused_ack.store(false, Ordering::Release);
                self.paused.store(false, Ordering::Release);
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "terminal input pump did not pause before child terminal handoff",
                ));
            }
            thread::sleep(TERMINAL_INPUT_CHILD_PAUSE_POLL_INTERVAL);
        }
        self.mark_alive();
        Ok(())
    }

    pub(super) fn resume_after_child_terminal(&self) {
        self.paused_ack.store(false, Ordering::Release);
        self.paused.store(false, Ordering::Release);
        self.mark_alive();
    }

    /// Replace a wedged pump thread with a freshly spawned one.
    ///
    /// The old thread may be blocked forever inside crossterm's blocking
    /// `event::read` (a stalled Windows console poll, or a Unix tty that
    /// stopped delivering bytes), so it can never be joined. Instead it is
    /// detached: `stop` is flagged and the `JoinHandle` dropped, so if the
    /// thread ever wakes it exits on its own (its send fails once `rx` is
    /// replaced, and the stop flag covers the poll loop).
    pub(super) fn restart_detached(&mut self) -> io::Result<()> {
        self.detach_current_thread();
        let parts = Self::spawn_parts()?;
        self.install_parts(parts);
        Ok(())
    }

    /// Flag the current pump thread to stop and drop its handle without
    /// joining (the thread may be wedged in a blocking terminal read).
    pub(super) fn detach_current_thread(&mut self) {
        self.stop.store(true, Ordering::Release);
        let _ = self.handle.take();
    }

    /// Adopt freshly spawned pump parts and reset the liveness clock.
    pub(super) fn install_parts(&mut self, parts: TerminalInputPumpParts) {
        self.rx = parts.rx;
        self.stop = parts.stop;
        self.paused = parts.paused;
        self.paused_ack = parts.paused_ack;
        self.handle = Some(parts.handle);
        self.last_alive_at.set(Instant::now());
    }
}

impl Drop for TerminalInputPump {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.handle.take() {
            #[cfg(target_os = "windows")]
            {
                drop(handle);
            }
            #[cfg(not(target_os = "windows"))]
            let _ = handle.join();
        }
    }
}

pub(super) fn engine_drain_budget_exhausted(
    events_drained: usize,
    started: Instant,
    now: Instant,
) -> bool {
    events_drained >= MAX_ENGINE_EVENTS_PER_DRAIN
        || now.saturating_duration_since(started) >= ENGINE_DRAIN_TIME_BUDGET
}
