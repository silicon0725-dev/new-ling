//! 极简 `Future` 阻塞执行器（std-only，避免引入 pollster 等额外依赖）。
//!
//! wgpu 的 `request_adapter` / `request_device` 返回 `Future`，但结果由
//! 内部工作线程唤醒；这里实现一个基于 `std::thread::park` 的 waker：
//! - 被唤醒时置位原子标志再 `unpark`，主循环据标志判断是否发生「丢失唤醒」；
//! - `park_timeout` 兜底，即使唤醒信号彻底丢失也能继续轮询。

use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

/// 带原子「已唤醒」标志的线程 waker
struct ThreadWaker {
    thread: std::thread::Thread,
    notified: std::sync::atomic::AtomicBool,
}

impl Wake for ThreadWaker {
    fn wake(self: Arc<Self>) {
        self.notified
            .store(true, std::sync::atomic::Ordering::Release);
        self.thread.unpark();
    }
    fn wake_by_ref(self: &Arc<Self>) {
        self.notified
            .store(true, std::sync::atomic::Ordering::Release);
        self.thread.unpark();
    }
}

/// 阻塞运行一个 `Future` 直至完成（仅供渲染器初始化 / 读回同步使用）
pub(crate) fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = std::pin::pin!(future);
    let waker_handle = Arc::new(ThreadWaker {
        thread: std::thread::current(),
        notified: std::sync::atomic::AtomicBool::new(false),
    });
    let waker = Waker::from(waker_handle.clone());
    let mut cx = Context::from_waker(&waker);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(output) => return output,
            Poll::Pending => {
                // 先检查标志再 park：若 wake 已发生则立即重轮询，避免丢失唤醒
                if !waker_handle
                    .notified
                    .swap(false, std::sync::atomic::Ordering::AcqRel)
                {
                    std::thread::park_timeout(std::time::Duration::from_millis(100));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runs_ready_future_immediately() {
        assert_eq!(block_on(async { 1 + 1 }), 2);
    }

    #[test]
    fn parks_and_resumes_on_wake_from_other_thread() {
        // 跨线程唤醒：另一个线程 sleep 后唤醒 waker，主线程必须能继续推进
        let value = block_on(async {
            struct Once {
                woken: std::sync::Arc<std::sync::atomic::AtomicUsize>,
                state: usize,
            }
            impl Future for Once {
                type Output = u32;
                fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<u32> {
                    match self.state {
                        0 => {
                            self.state = 1;
                            let waker = cx.waker().clone();
                            let flag = self.woken.clone();
                            std::thread::spawn(move || {
                                std::thread::sleep(std::time::Duration::from_millis(20));
                                flag.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                                waker.wake();
                            });
                            Poll::Pending
                        }
                        _ => Poll::Ready(7),
                    }
                }
            }
            Once {
                woken: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
                state: 0,
            }
            .await
        });
        assert_eq!(value, 7);
    }
}
