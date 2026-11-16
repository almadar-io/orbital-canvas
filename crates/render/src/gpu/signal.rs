//! A one-shot value that works as a future, so a wgpu callback can be awaited natively and in the browser.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll, Waker};

struct State<T> {
    value: Option<T>,
    closed: bool,
    waker: Option<Waker>,
}

pub(super) struct Sender<T>(Arc<Mutex<State<T>>>);
pub(super) struct Receiver<T>(Arc<Mutex<State<T>>>);

pub(super) fn signal<T>() -> (Sender<T>, Receiver<T>) {
    let state = Arc::new(Mutex::new(State {
        value: None,
        closed: false,
        waker: None,
    }));
    (Sender(state.clone()), Receiver(state))
}

impl<T> Sender<T> {
    pub(super) fn send(self, value: T) {
        let mut state = self.0.lock().unwrap_or_else(|p| p.into_inner());
        state.value = Some(value);
    }
}

impl<T> Drop for Sender<T> {
    fn drop(&mut self) {
        let mut state = self.0.lock().unwrap_or_else(|p| p.into_inner());
        state.closed = true;
        if let Some(waker) = state.waker.take() {
            waker.wake();
        }
    }
}

impl<T> Receiver<T> {
    /// A value was sent, or the sender went away: awaiting the receiver will not wait.
    #[cfg(not(target_arch = "wasm32"))]
    pub(super) fn settled(&self) -> bool {
        let state = self.0.lock().unwrap_or_else(|p| p.into_inner());
        state.value.is_some() || state.closed
    }
}

/// The sender went away without sending.
pub(super) struct Dropped;

impl<T> Future for Receiver<T> {
    type Output = Result<T, Dropped>;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let mut state = self.0.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(value) = state.value.take() {
            return Poll::Ready(Ok(value));
        }
        if state.closed {
            return Poll::Ready(Err(Dropped));
        }
        state.waker = Some(cx.waker().clone());
        Poll::Pending
    }
}
