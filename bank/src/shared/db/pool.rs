//! Adapter-owned slots include opening and closing connections. Failed opens are
//! reported to their initiating waiter through a oneshot; after cancellation the
//! error is discarded, but the slot is still freed. Warm-up polls every 2 ms.
use std::fmt::Display;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{Notify, oneshot};

pub(crate) trait Connector: Send + Sync + 'static {
    type Conn: Send + 'static;
    type Error: Display + Send + 'static;
    fn open(&self) -> Result<Self::Conn, Self::Error>;
    fn close(&self, conn: Self::Conn);
}

type OpenResult<E> = oneshot::Receiver<Result<(), PoolError<E>>>;

type Spawner = Box<dyn Fn(Box<dyn FnOnce() + Send>) -> std::io::Result<()> + Send + Sync>;

#[derive(Debug)]
pub(crate) enum PoolError<E> {
    Panicked,
    Busy,
    Closed,
    Open(E),
    Spawn(std::io::Error),
}

struct State<T> {
    idle: Vec<T>,
    pending_close: Vec<T>,
    slots: usize,
    closed: bool,
    draining: bool,
}

struct Inner<C: Connector> {
    connector: Arc<C>,
    state: Mutex<State<C::Conn>>,
    notify: Notify,
    max: usize,
    spawn: Spawner,
}

pub(crate) struct Pool<C: Connector> {
    inner: Arc<Inner<C>>,
}

impl<C: Connector> Pool<C> {
    pub(crate) fn new(connector: C, max: usize) -> Self {
        Self::with_spawner(
            connector,
            max,
            Box::new(|job| {
                std::thread::Builder::new()
                    .name("db-pool".into())
                    .spawn(job)
                    .map(|_| ())
            }),
        )
    }

    fn with_spawner(connector: C, max: usize, spawn: Spawner) -> Self {
        Self {
            inner: Arc::new(Inner {
                connector: Arc::new(connector),
                state: Mutex::new(State {
                    idle: Vec::new(),
                    pending_close: Vec::new(),
                    slots: 0,
                    closed: false,
                    draining: false,
                }),
                notify: Notify::new(),
                max,
                spawn,
            }),
        }
    }

    pub(crate) async fn acquire(&self, budget: Duration) -> Result<Lease<C>, PoolError<C::Error>> {
        let deadline = tokio::time::Instant::now() + budget;
        let mut opening = None;
        loop {
            let notified = self.inner.notify.notified();
            tokio::pin!(notified);
            let mut tried_drainer = false;
            loop {
                let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
                if state.closed {
                    return Err(PoolError::Closed);
                }
                // A zero budget remains an immediate idle-only attempt.
                if !budget.is_zero() && tokio::time::Instant::now() >= deadline {
                    let has_idle = !state.idle.is_empty();
                    drop(state);
                    // This waiter may have consumed the release notification.
                    if has_idle {
                        self.inner.notify.notify_one();
                    }
                    return Err(PoolError::Busy);
                }
                if let Some(conn) = state.idle.pop() {
                    return Ok(Lease {
                        inner: self.inner.clone(),
                        conn: Some(conn),
                        blocking: false,
                    });
                }
                if tokio::time::Instant::now() >= deadline {
                    return Err(PoolError::Busy);
                }
                notified.as_mut().enable();
                if state.slots == self.inner.max
                    && !state.pending_close.is_empty()
                    && !state.draining
                    && !tried_drainer
                {
                    state.draining = true;
                    drop(state);
                    self.inner.start_drainer();
                    // Recheck idle, closure and deadline after releasing the lock.
                    // Keep the registered notification, and wait before retrying
                    // a failed drainer spawn so it cannot spin on pending closes.
                    tried_drainer = true;
                    continue;
                }
                if state.slots < self.inner.max && opening.is_none() {
                    state.slots += 1;
                    drop(state);
                    opening = Some(self.inner.start_open()?);
                }
                break;
            }
            if let Some(rx) = opening.as_mut() {
                tokio::select! {
                    biased;
                    _ = &mut notified => {},
                    result = rx => {
                        opening = None;
                        if let Ok(Err(error)) = result { return Err(error); }
                    },
                    _ = tokio::time::sleep_until(deadline) => return Err(PoolError::Busy),
                }
            } else if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return Err(PoolError::Busy);
            }
        }
    }

    pub(crate) fn warm(&self, n: usize, budget: Duration) -> Result<(), PoolError<C::Error>> {
        let target = n.min(self.inner.max);
        let deadline = Instant::now() + budget;
        let mut opens: Vec<OpenResult<C::Error>> = Vec::new();
        loop {
            let mut index = 0;
            while index < opens.len() {
                match opens[index].try_recv() {
                    Ok(Err(error)) => return Err(error),
                    Ok(Ok(())) | Err(oneshot::error::TryRecvError::Closed) => {
                        drop(opens.swap_remove(index));
                    }
                    Err(oneshot::error::TryRecvError::Empty) => index += 1,
                }
            }
            let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            if state.closed {
                return Err(PoolError::Closed);
            }
            if state.idle.len() >= target {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(PoolError::Busy);
            }
            if state.slots < target {
                state.slots += 1;
                drop(state);
                opens.push(self.inner.start_open()?);
            } else {
                drop(state);
                std::thread::sleep(Duration::from_millis(2));
            }
        }
    }

    pub(crate) fn close(&self) {
        let idle = {
            let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
            state.closed = true;
            std::mem::take(&mut state.idle)
        };
        self.inner.notify.notify_waiters();
        for conn in idle {
            self.inner.retire(conn, false);
        }
    }
}

impl<C: Connector> Drop for Pool<C> {
    fn drop(&mut self) {
        self.close();
    }
}

impl<C: Connector> Inner<C> {
    fn free_slot(&self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.slots -= 1;
        drop(state);
        self.notify.notify_waiters();
    }

    fn start_open(self: &Arc<Self>) -> Result<OpenResult<C::Error>, PoolError<C::Error>> {
        let inner = self.clone();
        let (tx, rx) = oneshot::channel();
        if let Err(error) = (self.spawn)(Box::new(move || {
            match catch_unwind(AssertUnwindSafe(|| inner.connector.open())) {
                Ok(Ok(conn)) => {
                    let mut state = inner.state.lock().unwrap_or_else(|e| e.into_inner());
                    if state.closed {
                        drop(state);
                        inner.close_blocking(conn);
                    } else {
                        state.idle.push(conn);
                        drop(state);
                        inner.notify.notify_waiters();
                    }
                    let _ = tx.send(Ok(()));
                }
                result => {
                    let error = match result {
                        Ok(Err(error)) => PoolError::Open(error),
                        Err(_) => PoolError::Panicked,
                        Ok(Ok(_)) => unreachable!(),
                    };
                    // Send before notifying so the initiating caller observes the error.
                    let _ = tx.send(Err(error));
                    inner.free_slot();
                }
            }
        })) {
            self.free_slot();
            return Err(PoolError::Spawn(error));
        }
        Ok(rx)
    }

    fn close_blocking(&self, conn: C::Conn) {
        let _ = catch_unwind(AssertUnwindSafe(|| self.connector.close(conn)));
        self.free_slot();
        self.drain(false);
    }

    fn drain(&self, drainer: bool) {
        loop {
            let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
            match state.pending_close.pop() {
                Some(conn) => {
                    drop(state);
                    let _ = catch_unwind(AssertUnwindSafe(|| self.connector.close(conn)));
                    self.free_slot();
                }
                None => {
                    if drainer {
                        state.draining = false;
                    }
                    break;
                }
            }
        }
    }

    fn start_drainer(self: &Arc<Self>) {
        let inner = self.clone();
        if (self.spawn)(Box::new(move || inner.drain(true))).is_err() {
            self.state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .draining = false;
        }
    }

    fn retire(self: &Arc<Self>, conn: C::Conn, blocking: bool) {
        // Retain ownership when a spawner rejects and drops the job.
        let owned = Arc::new(Mutex::new(Some(conn)));
        let job_conn = owned.clone();
        let inner = self.clone();
        if let Err(error) = (self.spawn)(Box::new(move || {
            let conn = job_conn.lock().unwrap_or_else(|e| e.into_inner()).take();
            if let Some(conn) = conn {
                inner.close_blocking(conn);
            }
        })) {
            let conn = owned
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take()
                .expect("unstarted retirement");
            if blocking {
                self.close_blocking(conn);
            } else {
                self.state
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .pending_close
                    .push(conn);
                tracing::warn!(%error, "retirement thread unavailable; close deferred");
                // Waiters re-check and may start a drainer for the queued close.
                self.notify.notify_waiters();
            }
        }
    }
}

impl<C: Connector> Drop for Inner<C> {
    fn drop(&mut self) {
        let state = self.state.get_mut().unwrap_or_else(|e| e.into_inner());
        state.closed = true;
        let connections: Vec<_> = state
            .idle
            .drain(..)
            .chain(state.pending_close.drain(..))
            .collect();
        if connections.is_empty() {
            return;
        }
        let owned = Arc::new(Mutex::new(connections));
        let job_owned = owned.clone();
        let connector = self.connector.clone();
        if (self.spawn)(Box::new(move || {
            for conn in job_owned
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .drain(..)
            {
                let _ = catch_unwind(AssertUnwindSafe(|| connector.close(conn)));
            }
        }))
        .is_err()
        {
            // Residual case: if no thread can be spawned, final cleanup runs inline.
            for conn in owned.lock().unwrap_or_else(|e| e.into_inner()).drain(..) {
                let _ = catch_unwind(AssertUnwindSafe(|| self.connector.close(conn)));
            }
        }
    }
}

type DetachedOpen<C> =
    oneshot::Receiver<Result<<C as Connector>::Conn, PoolError<<C as Connector>::Error>>>;

pub(crate) fn open_detached<C: Connector>(
    connector: Arc<C>,
) -> Result<DetachedOpen<C>, std::io::Error> {
    let (tx, rx) = oneshot::channel();
    std::thread::Builder::new()
        .name("db-open".into())
        .spawn(move || {
            let result = match catch_unwind(AssertUnwindSafe(|| connector.open())) {
                Ok(result) => result.map_err(PoolError::Open),
                Err(_) => Err(PoolError::Panicked),
            };
            if let Err(Ok(conn)) = tx.send(result) {
                let _ = catch_unwind(AssertUnwindSafe(|| connector.close(conn)));
            }
        })?;
    Ok(rx)
}

/// Called on a blocking worker; spawn failure falls back to that worker.
pub(crate) fn close_detached<C: Connector>(connector: Arc<C>, conn: C::Conn) {
    let owned = Arc::new(Mutex::new(Some(conn)));
    let job_owned = owned.clone();
    let job_connector = connector.clone();
    if std::thread::Builder::new()
        .name("db-close".into())
        .spawn(move || {
            if let Some(conn) = job_owned.lock().unwrap_or_else(|e| e.into_inner()).take() {
                let _ = catch_unwind(AssertUnwindSafe(|| job_connector.close(conn)));
            }
        })
        .is_err()
        && let Some(conn) = owned.lock().unwrap_or_else(|e| e.into_inner()).take()
    {
        let _ = catch_unwind(AssertUnwindSafe(|| connector.close(conn)));
    }
}

pub(crate) struct Lease<C: Connector> {
    inner: Arc<Inner<C>>,
    conn: Option<C::Conn>,
    blocking: bool,
}

impl<C: Connector> Lease<C> {
    /// Called only after moving the lease onto the adapter's blocking worker.
    pub(crate) fn mark_blocking(&mut self) {
        self.blocking = true;
    }
    pub(crate) fn conn(&self) -> &C::Conn {
        self.conn.as_ref().expect("active lease")
    }
    pub(crate) fn conn_mut(&mut self) -> &mut C::Conn {
        self.conn.as_mut().expect("active lease")
    }
    pub(crate) fn release(mut self, healthy: bool) {
        let conn = self.conn.take().expect("active lease");
        let mut state = self.inner.state.lock().unwrap_or_else(|e| e.into_inner());
        if healthy && !state.closed {
            state.idle.push(conn);
            drop(state);
            self.inner.notify.notify_one();
        } else {
            drop(state);
            self.inner.retire(
                conn,
                self.blocking || tokio::runtime::Handle::try_current().is_err(),
            );
        }
    }
}

impl<C: Connector> Drop for Lease<C> {
    fn drop(&mut self) {
        if let Some(conn) = self.conn.take() {
            self.inner.retire(
                conn,
                self.blocking || tokio::runtime::Handle::try_current().is_err(),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    struct Gate {
        blocked: AtomicBool,
        sender: std::sync::mpsc::Sender<()>,
        receiver: Mutex<std::sync::mpsc::Receiver<()>>,
    }

    impl Default for Gate {
        fn default() -> Self {
            let (sender, receiver) = std::sync::mpsc::channel();
            Self {
                blocked: AtomicBool::new(false),
                sender,
                receiver: Mutex::new(receiver),
            }
        }
    }

    #[derive(Clone, Default)]
    struct Fake {
        open_gate: Arc<Gate>,
        close_gate: Arc<Gate>,
        open_delay: Duration,
        close_delay: Duration,
        fail: Arc<AtomicBool>,
        panic_open: Arc<AtomicBool>,
        panic_close: Arc<AtomicBool>,
        live: Arc<AtomicUsize>,
        peak: Arc<AtomicUsize>,
        opened: Arc<AtomicUsize>,
        closed: Arc<AtomicUsize>,
    }

    fn wait(gate: &Gate) {
        if gate.blocked.load(Ordering::SeqCst) {
            gate.receiver
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .recv()
                .unwrap();
        }
    }

    fn unblock(gate: &Gate) {
        gate.blocked.store(false, Ordering::SeqCst);
        gate.sender.send(()).unwrap();
    }

    impl Connector for Fake {
        type Conn = usize;
        type Error = &'static str;
        fn open(&self) -> Result<usize, &'static str> {
            assert!(!self.panic_open.load(Ordering::SeqCst), "open panic");
            self.opened.fetch_add(1, Ordering::SeqCst);
            let live = self.live.fetch_add(1, Ordering::SeqCst) + 1;
            self.peak.fetch_max(live, Ordering::SeqCst);
            std::thread::sleep(self.open_delay);
            wait(&self.open_gate);
            if self.fail.load(Ordering::SeqCst) {
                self.live.fetch_sub(1, Ordering::SeqCst);
                Err("open failed")
            } else {
                Ok(live)
            }
        }
        fn close(&self, _: usize) {
            std::thread::sleep(self.close_delay);
            wait(&self.close_gate);
            self.live.fetch_sub(1, Ordering::SeqCst);
            self.closed.fetch_add(1, Ordering::SeqCst);
            assert!(
                !self.panic_close.swap(false, Ordering::SeqCst),
                "close panic"
            );
        }
    }

    const BUDGET: Duration = Duration::from_secs(2);
    const SHORT: Duration = Duration::from_millis(50);

    async fn until(mut condition: impl FnMut() -> bool) {
        tokio::time::timeout(BUDGET, async {
            while !condition() {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
        })
        .await
        .expect("condition within budget");
    }

    fn slots(pool: &Pool<Fake>) -> usize {
        pool.inner
            .state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .slots
    }

    fn failing_pool(fake: Fake, fail: Arc<AtomicBool>) -> Pool<Fake> {
        Pool::with_spawner(
            fake,
            2,
            Box::new(move |job| {
                if fail.load(Ordering::SeqCst) {
                    Err(std::io::Error::other("spawn failed"))
                } else {
                    std::thread::Builder::new().spawn(job).map(|_| ())
                }
            }),
        )
    }

    #[tokio::test]
    async fn expired_waiter_leaves_returned_connection_idle() {
        use std::future::Future;
        use std::task::{Context, Poll, Waker};

        let pool = Pool::new(Fake::default(), 1);
        let lease = pool.acquire(BUDGET).await.unwrap();
        let mut acquire = Box::pin(pool.acquire(SHORT));
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(acquire.as_mut().poll(&mut context), Poll::Pending));
        // Do not poll the waiter again until both its deadline and notify are ready.
        std::thread::sleep(SHORT);
        lease.release(true);
        assert!(matches!(
            acquire.as_mut().poll(&mut context),
            Poll::Ready(Err(PoolError::Busy))
        ));
        assert_eq!(pool.inner.state.lock().unwrap().idle.len(), 1);
        pool.acquire(BUDGET).await.unwrap().release(true);
    }

    #[tokio::test]
    async fn expired_waiter_forwards_notification_to_live_waiter() {
        use std::future::Future;
        use std::task::{Context, Poll, Waker};

        let pool = Pool::new(Fake::default(), 1);
        let lease = pool.acquire(BUDGET).await.unwrap();
        let mut expired = Box::pin(pool.acquire(SHORT));
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(expired.as_mut().poll(&mut context), Poll::Pending));
        // Expire the first waiter without polling it, then register a live one.
        std::thread::sleep(SHORT);
        let mut live = Box::pin(pool.acquire(BUDGET));
        assert!(matches!(live.as_mut().poll(&mut context), Poll::Pending));
        lease.release(true);
        // The single release selected the first waiter, not the live one.
        assert!(matches!(live.as_mut().poll(&mut context), Poll::Pending));
        assert!(matches!(
            expired.as_mut().poll(&mut context),
            Poll::Ready(Err(PoolError::Busy))
        ));
        assert_eq!(pool.inner.state.lock().unwrap().idle.len(), 1);
        let Poll::Ready(Ok(lease)) = live.as_mut().poll(&mut context) else {
            panic!("live waiter must acquire immediately without another release");
        };
        lease.release(true);
    }

    #[tokio::test]
    async fn zero_budget_only_takes_an_existing_idle_connection() {
        let fake = Fake::default();
        let pool = Pool::new(fake.clone(), 1);
        assert!(matches!(
            pool.acquire(Duration::ZERO).await,
            Err(PoolError::Busy)
        ));
        assert_eq!(fake.opened.load(Ordering::SeqCst), 0);
        pool.acquire(BUDGET).await.unwrap().release(true);
        pool.acquire(Duration::ZERO).await.unwrap().release(true);
    }

    // Run the drainer synchronously, then force the event while acquire has
    // released the state lock but has not reacquired it to reserve an opener.
    fn deferred_pool(after_drain: impl Fn() + Send + Sync + 'static) -> (Pool<Fake>, Fake) {
        let fake = Fake::default();
        let pool = Pool::with_spawner(
            fake.clone(),
            1,
            Box::new(move |job| {
                job();
                after_drain();
                Ok(())
            }),
        );
        let conn = fake.open().unwrap();
        {
            let mut state = pool.inner.state.lock().unwrap();
            state.slots = 1;
            state.pending_close.push(conn);
        }
        (pool, fake)
    }

    #[tokio::test]
    async fn idle_returned_during_drainer_start_prevents_open() {
        let returned = Arc::new(Mutex::new(None::<Lease<Fake>>));
        let during_start = returned.clone();
        let spawns = Arc::new(AtomicUsize::new(0));
        let attempts = spawns.clone();
        let fake = Fake::default();
        let pool = Pool::with_spawner(
            fake.clone(),
            2,
            Box::new(move |job| {
                attempts.fetch_add(1, Ordering::SeqCst);
                job();
                if let Some(lease) = during_start.lock().unwrap().take() {
                    lease.release(true);
                }
                Ok(())
            }),
        );
        let retired = fake.open().unwrap();
        let healthy = fake.open().unwrap();
        {
            let mut state = pool.inner.state.lock().unwrap();
            state.slots = 2;
            state.pending_close.push(retired);
        }
        *returned.lock().unwrap() = Some(Lease {
            inner: pool.inner.clone(),
            conn: Some(healthy),
            blocking: false,
        });
        let lease = pool.acquire(BUDGET).await.unwrap();
        assert_eq!(*lease.conn(), healthy);
        assert_eq!(spawns.load(Ordering::SeqCst), 1);
        assert_eq!(fake.opened.load(Ordering::SeqCst), 2);
        lease.release(true);
    }

    #[tokio::test]
    async fn failed_drainer_spawn_waits_without_spinning() {
        use std::future::Future;
        use std::task::{Context, Poll, Waker};

        let spawns = Arc::new(AtomicUsize::new(0));
        let attempts = spawns.clone();
        let fake = Fake::default();
        let pool = Pool::with_spawner(
            fake.clone(),
            1,
            Box::new(move |_| {
                attempts.fetch_add(1, Ordering::SeqCst);
                Err(std::io::Error::other("spawn failed"))
            }),
        );
        {
            let mut state = pool.inner.state.lock().unwrap();
            state.slots = 1;
            state.pending_close.push(fake.open().unwrap());
        }
        let mut acquire = Box::pin(pool.acquire(SHORT));
        let mut context = Context::from_waker(Waker::noop());
        assert!(matches!(acquire.as_mut().poll(&mut context), Poll::Pending));
        assert!(matches!(acquire.as_mut().poll(&mut context), Poll::Pending));
        assert_eq!(spawns.load(Ordering::SeqCst), 1);
        assert!(matches!(acquire.await, Err(PoolError::Busy)));
        assert_eq!(spawns.load(Ordering::SeqCst), 1);
        assert_eq!(slots(&pool), 1);
    }

    #[tokio::test]
    async fn shutdown_during_drainer_start_does_not_reserve_opener() {
        let owner = Arc::new(Mutex::new(std::sync::Weak::<Pool<Fake>>::new()));
        let closer = owner.clone();
        let (pool, fake) = deferred_pool(move || {
            closer.lock().unwrap().upgrade().unwrap().close();
        });
        let pool = Arc::new(pool);
        *owner.lock().unwrap() = Arc::downgrade(&pool);
        assert!(matches!(pool.acquire(BUDGET).await, Err(PoolError::Closed)));
        assert_eq!(fake.opened.load(Ordering::SeqCst), 1);
        assert_eq!(fake.closed.load(Ordering::SeqCst), 1);
        assert_eq!(slots(&pool), 0);
    }

    #[tokio::test]
    async fn deadline_during_drainer_start_does_not_reserve_opener() {
        let (pool, fake) = deferred_pool(|| std::thread::sleep(SHORT));
        assert!(matches!(pool.acquire(SHORT).await, Err(PoolError::Busy)));
        assert_eq!(fake.opened.load(Ordering::SeqCst), 1);
        assert_eq!(fake.closed.load(Ordering::SeqCst), 1);
        assert_eq!(slots(&pool), 0);
    }

    #[tokio::test]
    async fn panicking_open_frees_slot() {
        let fake = Fake::default();
        fake.panic_open.store(true, Ordering::SeqCst);
        let pool = Pool::new(fake, 1);
        assert!(matches!(
            tokio::time::timeout(SHORT, pool.acquire(BUDGET))
                .await
                .unwrap(),
            Err(PoolError::Panicked)
        ));
        until(|| slots(&pool) == 0).await;
    }

    #[tokio::test]
    async fn panicking_close_drains_pending() {
        let fake = Fake::default();
        let fail = Arc::new(AtomicBool::new(false));
        let pool = failing_pool(fake.clone(), fail.clone());
        let first = pool.acquire(BUDGET).await.unwrap();
        let second = pool.acquire(BUDGET).await.unwrap();
        fail.store(true, Ordering::SeqCst);
        drop(first);
        assert_eq!(slots(&pool), 2);
        fake.panic_close.store(true, Ordering::SeqCst);
        fail.store(false, Ordering::SeqCst);
        drop(second);
        until(|| slots(&pool) == 0).await;
        assert_eq!(fake.closed.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn acquire_retries_deferred_retirement() {
        let fake = Fake::default();
        let fail = Arc::new(AtomicBool::new(false));
        let job_fail = fail.clone();
        let pool = Pool::with_spawner(
            fake.clone(),
            1,
            Box::new(move |job| {
                if job_fail.load(Ordering::SeqCst) {
                    Err(std::io::Error::other("spawn failed"))
                } else {
                    std::thread::Builder::new().spawn(job).map(|_| ())
                }
            }),
        );
        let lease = pool.acquire(BUDGET).await.unwrap();
        fail.store(true, Ordering::SeqCst);
        drop(lease);
        assert_eq!(slots(&pool), 1);
        fail.store(false, Ordering::SeqCst);
        pool.acquire(BUDGET).await.unwrap().release(true);
        assert_eq!(fake.closed.load(Ordering::SeqCst), 1);
        assert_eq!(fake.opened.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn idle_pool_drop_does_not_wait_for_close() {
        let fake = Fake::default();
        fake.close_gate.blocked.store(true, Ordering::SeqCst);
        let pool = Pool::new(fake.clone(), 1);
        pool.acquire(BUDGET).await.unwrap().release(true);
        let start = Instant::now();
        drop(pool);
        assert!(start.elapsed() < SHORT);
        assert_eq!(fake.closed.load(Ordering::SeqCst), 0);
        unblock(&fake.close_gate);
        until(|| fake.closed.load(Ordering::SeqCst) == 1).await;
    }

    #[tokio::test]
    async fn detached_connections_use_dedicated_threads() {
        struct ThreadConnector {
            caller: std::thread::ThreadId,
            closed: std::sync::mpsc::Sender<bool>,
        }
        impl Connector for ThreadConnector {
            type Conn = ();
            type Error = &'static str;
            fn open(&self) -> Result<(), Self::Error> {
                assert_ne!(std::thread::current().id(), self.caller);
                assert_eq!(std::thread::current().name(), Some("db-open"));
                Ok(())
            }
            fn close(&self, _: ()) {
                self.closed
                    .send(std::thread::current().name() == Some("db-close"))
                    .unwrap();
            }
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let connector = Arc::new(ThreadConnector {
            caller: std::thread::current().id(),
            closed: tx,
        });
        open_detached(connector.clone())
            .unwrap()
            .await
            .unwrap()
            .unwrap();
        tokio::task::spawn_blocking(move || close_detached(connector, ()))
            .await
            .unwrap();
        assert!(rx.recv_timeout(BUDGET).unwrap());
    }

    #[tokio::test]
    async fn last_pool_drop_does_not_wait_for_close() {
        let fake = Fake::default();
        fake.close_gate.blocked.store(true, Ordering::SeqCst);
        let fail = Arc::new(AtomicBool::new(false));
        let pool = failing_pool(fake.clone(), fail.clone());
        pool.acquire(BUDGET).await.unwrap().release(true);
        // Queue retirement first so final Inner::drop owns the deferred connection.
        fail.store(true, Ordering::SeqCst);
        pool.close();
        fail.store(false, Ordering::SeqCst);
        let start = Instant::now();
        drop(pool);
        assert!(start.elapsed() < SHORT);
        assert_eq!(fake.closed.load(Ordering::SeqCst), 0);
        unblock(&fake.close_gate);
        until(|| fake.closed.load(Ordering::SeqCst) == 1).await;
    }

    #[tokio::test]
    async fn slot_bound_under_mixed_load() {
        let fake = Fake::default();
        let pool = Arc::new(Pool::new(fake.clone(), 3));
        let mut tasks = Vec::new();
        for i in 0..60 {
            let pool = pool.clone();
            tasks.push(tokio::spawn(async move {
                let lease = pool.acquire(BUDGET).await.unwrap();
                tokio::time::sleep(Duration::from_millis(2)).await;
                lease.release(i % 3 != 0);
            }));
        }
        for task in tasks {
            task.await.unwrap();
        }
        assert!(fake.peak.load(Ordering::SeqCst) <= 3);
        assert!(slots(&pool) <= 3);
    }

    #[tokio::test]
    async fn waits_for_a_return_within_budget() {
        let pool = Arc::new(Pool::new(Fake::default(), 1));
        let lease = pool.acquire(BUDGET).await.unwrap();
        let waiter = {
            let pool = pool.clone();
            tokio::spawn(async move { pool.acquire(BUDGET).await })
        };
        tokio::time::sleep(SHORT).await;
        assert!(!waiter.is_finished());
        lease.release(true);
        waiter.await.unwrap().unwrap().release(true);
    }

    #[tokio::test]
    async fn late_open_lands_in_idle() {
        let fake = Fake {
            open_delay: Duration::from_millis(80),
            ..Fake::default()
        };
        fake.open_gate.blocked.store(true, Ordering::SeqCst);
        let pool = Pool::new(fake.clone(), 1);
        assert!(matches!(pool.acquire(SHORT).await, Err(PoolError::Busy)));
        unblock(&fake.open_gate);
        until(|| {
            !pool
                .inner
                .state
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .idle
                .is_empty()
        })
        .await;
        let mut lease = pool.acquire(BUDGET).await.unwrap();
        assert_eq!(*lease.conn_mut(), 1);
        lease.release(true);
        assert_eq!(fake.opened.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn failed_open_frees_slot() {
        let fake = Fake::default();
        fake.fail.store(true, Ordering::SeqCst);
        let pool = Pool::new(fake.clone(), 1);
        assert!(matches!(
            pool.acquire(BUDGET).await,
            Err(PoolError::Open("open failed"))
        ));
        until(|| slots(&pool) == 0).await;
        fake.fail.store(false, Ordering::SeqCst);
        pool.acquire(BUDGET).await.unwrap().release(true);
    }

    #[tokio::test]
    async fn late_failed_open_frees_slot() {
        let fake = Fake::default();
        fake.fail.store(true, Ordering::SeqCst);
        fake.open_gate.blocked.store(true, Ordering::SeqCst);
        let pool = Pool::new(fake.clone(), 1);
        assert!(matches!(pool.acquire(SHORT).await, Err(PoolError::Busy)));
        assert_eq!(slots(&pool), 1);
        unblock(&fake.open_gate);
        until(|| slots(&pool) == 0).await;
    }

    #[tokio::test]
    async fn stalled_open_bounds_wait() {
        let fake = Fake::default();
        fake.open_gate.blocked.store(true, Ordering::SeqCst);
        let pool = Pool::new(fake.clone(), 1);
        let start = Instant::now();
        assert!(matches!(pool.acquire(SHORT).await, Err(PoolError::Busy)));
        assert!(start.elapsed() < Duration::from_secs(1));
        assert_eq!(slots(&pool), 1);
        pool.close();
        unblock(&fake.open_gate);
        until(|| slots(&pool) == 0).await;
    }

    #[tokio::test]
    async fn stalled_close_keeps_slot() {
        let fake = Fake {
            close_delay: Duration::from_millis(5),
            ..Fake::default()
        };
        fake.close_gate.blocked.store(true, Ordering::SeqCst);
        let pool = Pool::new(fake.clone(), 1);
        pool.acquire(BUDGET).await.unwrap().release(false);
        assert!(matches!(pool.acquire(SHORT).await, Err(PoolError::Busy)));
        assert_eq!(slots(&pool), 1);
        unblock(&fake.close_gate);
        until(|| slots(&pool) == 0).await;
        pool.acquire(BUDGET).await.unwrap().release(true);
    }

    #[tokio::test]
    async fn opener_spawn_failure_frees_slot() {
        let pool = failing_pool(Fake::default(), Arc::new(AtomicBool::new(true)));
        assert!(matches!(
            pool.acquire(BUDGET).await,
            Err(PoolError::Spawn(_))
        ));
        assert_eq!(slots(&pool), 0);
    }

    #[tokio::test]
    async fn retirement_spawn_failure_closes_inline_on_worker() {
        let fake = Fake::default();
        let fail = Arc::new(AtomicBool::new(false));
        let pool = failing_pool(fake.clone(), fail.clone());
        let mut lease = pool.acquire(BUDGET).await.unwrap();
        fail.store(true, Ordering::SeqCst);
        tokio::task::spawn_blocking(move || {
            lease.mark_blocking();
            lease.release(false);
        })
        .await
        .unwrap();
        assert_eq!(slots(&pool), 0);
        assert_eq!(fake.closed.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn panic_drops_and_retires_lease() {
        let fake = Fake::default();
        let pool = Pool::new(fake.clone(), 1);
        let lease = pool.acquire(BUDGET).await.unwrap();
        let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _lease = lease;
            panic!("work panicked");
        }));
        assert!(panic.is_err());
        until(|| slots(&pool) == 0).await;
        assert_eq!(fake.closed.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn close_returns_wakes_waiters_and_retires_late_arrivals() {
        let fake = Fake::default();
        let pool = Arc::new(Pool::new(fake.clone(), 3));
        let checked = pool.acquire(BUDGET).await.unwrap();
        let idle = pool.acquire(BUDGET).await.unwrap();
        fake.open_gate.blocked.store(true, Ordering::SeqCst);
        let waiter = {
            let pool = pool.clone();
            tokio::spawn(async move { pool.acquire(BUDGET).await })
        };
        until(|| slots(&pool) == 3).await;
        idle.release(true);
        // Ensure an idle, an opening and a checked-out connection coexist.
        let start = Instant::now();
        pool.close();
        assert!(start.elapsed() < SHORT);
        assert!(matches!(waiter.await.unwrap(), Err(PoolError::Closed)));
        assert!(matches!(pool.acquire(BUDGET).await, Err(PoolError::Closed)));
        checked.release(true);
        unblock(&fake.open_gate);
        until(|| slots(&pool) == 0).await;
        assert_eq!(fake.closed.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn failed_close_spawn_defers_without_freeing_slots() {
        let fake = Fake::default();
        let fail = Arc::new(AtomicBool::new(false));
        let pool = failing_pool(fake.clone(), fail.clone());
        let first = pool.acquire(BUDGET).await.unwrap();
        let checked = pool.acquire(BUDGET).await.unwrap();
        first.release(true);
        fail.store(true, Ordering::SeqCst);
        fake.close_gate.blocked.store(true, Ordering::SeqCst);
        let start = Instant::now();
        pool.close();
        assert!(start.elapsed() < SHORT);
        assert_eq!(slots(&pool), 2);
        assert_eq!(fake.closed.load(Ordering::SeqCst), 0);
        fail.store(false, Ordering::SeqCst);
        checked.release(true);
        unblock(&fake.close_gate);
        until(|| slots(&pool) == 0).await;
        assert_eq!(fake.closed.load(Ordering::SeqCst), 2);
    }
    #[tokio::test]
    async fn failed_rollback_after_success_retires_connection() {
        let fake = Fake::default();
        let pool = Pool::new(fake.clone(), 1);
        let lease = pool.acquire(BUDGET).await.unwrap();
        let (result, healthy) = super::super::work_and_rollback(
            &mut (),
            |_| Ok::<_, &str>(42),
            |_| Err("rollback failed"),
        )
        .unwrap();
        assert_eq!(result, Ok(42));
        assert!(!healthy);
        lease.release(healthy);
        until(|| slots(&pool) == 0).await;
        assert_eq!(fake.closed.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}
