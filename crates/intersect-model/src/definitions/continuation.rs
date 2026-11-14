//! Deterministic local suspension for definition traversals. No external wakeup.
use std::{
    future::Future,
    task::{Context, Poll, Waker},
};
pub(crate) async fn pause() {
    let mut yielded = false;
    std::future::poll_fn(move |_| {
        if yielded {
            Poll::Ready(())
        } else {
            yielded = true;
            Poll::Pending
        }
    })
    .await
}
/// Ordinary callers drain the same implementation used by retained cursors.
pub(crate) fn complete<T>(future: impl Future<Output = T>) -> T {
    let mut future = std::pin::pin!(future);
    let mut context = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(result) = future.as_mut().poll(&mut context) {
            return result;
        }
    }
}
