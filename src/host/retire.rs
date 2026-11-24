//! Frees a replaced scene's large values away from the frame thread on native hosts.

/// Drops `value` on a background thread on native hosts (a big scene tree takes milliseconds to free), in place on
/// the web, which has no thread to hand it to.
pub(crate) fn retire<T: Send + 'static>(value: T) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        use std::sync::{mpsc, Mutex, OnceLock};
        type Garbage = Box<dyn Send>;
        static BIN: OnceLock<Option<Mutex<mpsc::Sender<Garbage>>>> = OnceLock::new();
        let bin = BIN.get_or_init(|| {
            let (send, receive) = mpsc::channel::<Garbage>();
            std::thread::Builder::new()
                .name("canvas-retire".into())
                .spawn(move || receive.into_iter().for_each(drop))
                .ok()
                .map(|_| Mutex::new(send))
        });
        if let Some(bin) = bin {
            let sent = bin
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .send(Box::new(value));
            if let Err(mpsc::SendError(value)) = sent {
                drop(value);
            }
            return;
        }
    }
    drop(value);
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_retired_value_is_freed_off_the_calling_thread() {
        struct Marks(std::sync::mpsc::Sender<std::thread::ThreadId>);
        impl Drop for Marks {
            fn drop(&mut self) {
                let _ = self.0.send(std::thread::current().id());
            }
        }
        let (send, receive) = std::sync::mpsc::channel();
        super::retire(Marks(send.clone()));
        let freed_on = receive.recv_timeout(std::time::Duration::from_secs(5)).expect("it is freed");
        assert_ne!(freed_on, std::thread::current().id(), "the value is freed on another thread");
        drop(Marks(send));
        assert_eq!(receive.recv().unwrap(), std::thread::current().id(), "control: a plain drop frees it here");
    }
}
