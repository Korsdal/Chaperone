// SPDX-License-Identifier: Apache-2.0
// Copyright 2026 SerenIT ApS
// Copyright 2026 Prompted EV

//! In-process serialisation per canonical path (E-027, D-030).
//!
//! ## Why this exists
//!
//! `SessionId` is derived per **process** (`sess-{pid}`), so every subagent in one
//! Claude Desktop session shares one MCP server, one session, and one lease
//! identity. Coord's lease conflict check keys on `path` alone and does not
//! exempt the holder's own session, so two subagents writing one file produce
//! `LeaseHeld { holder: <the caller themself> }` — a session colliding with
//! itself and being told the file is taken by the user who is asking.
//!
//! That is not a data-integrity problem: nothing was written twice, and
//! invariant 3 never depended on the lease. It is an *agent-behaviour* problem,
//! and the sharp end of it is what the README's failure directions warn about —
//! an error is exactly what makes an LLM either abandon the task or retry
//! forever.
//!
//! The pilot's central workflow (a large fan-out of
//! parallel subagents, each updating a shared `case.yaml` after its stage) walks
//! straight into it, so this is the common path there rather than an edge case.
//!
//! ## What it does
//!
//! Queue locally *before* asking coord. Same-process contention becomes a short
//! wait instead of a collision, which costs latency — the cheap axis here, since
//! a salesperson running a workflow already expects it to take time — and buys
//! away a whole class of confusing failure.
//!
//! Cross-*process* contention (two people's laptops on one file) is the case
//! Chaperone genuinely exists for, and this does nothing about it. That is
//! handled by the bounded retry in [`crate::lease_manager`], which sits directly
//! above these locks.

use chapr_proto::CanonicalPath;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{Mutex, OwnedMutexGuard};

/// A held set of per-path locks. Dropping it releases them.
pub type PathGuards = Vec<OwnedMutexGuard<()>>;

/// One mutex per canonical path, created on demand.
#[derive(Default)]
pub struct PathLocks {
    /// Guards the map only — **never** held across an await, so a task waiting
    /// for a path cannot block a task looking one up.
    inner: Mutex<HashMap<CanonicalPath, Arc<Mutex<()>>>>,
}

impl PathLocks {
    pub fn new() -> Self {
        Self::default()
    }

    /// Lock every path in `paths`, waiting as long as it takes.
    ///
    /// **Acquired in canonical (sorted) order, de-duplicated** — the same rule
    /// coord applies to an all-or-none lease set, and for the same reason: two
    /// tasks locking `{a, b}` in opposite orders deadlock. `chapr_move` is the
    /// verb that makes this reachable, since it is the one that takes two paths.
    ///
    /// De-duplication matters as well as tidiness: `tokio::sync::Mutex` is not
    /// reentrant, so a set naming one path twice would deadlock against itself.
    pub async fn lock_all(&self, paths: &[CanonicalPath]) -> PathGuards {
        let mut wanted: Vec<CanonicalPath> = paths.to_vec();
        wanted.sort();
        wanted.dedup();

        // Take the Arcs under the map lock, then release it before awaiting any
        // of them.
        let locks: Vec<Arc<Mutex<()>>> = {
            let mut map = self.inner.lock().await;
            // Opportunistic prune: an entry the map alone still holds is not in
            // use by anybody, so it can go. Without this the map grows one entry
            // per distinct path for the life of the process. Checking for
            // `strong_count == 1` is what makes it safe — an entry someone holds
            // a guard for, or is about to await, always has a second Arc alive,
            // and dropping such an entry would let a newcomer create a *second*
            // mutex for the same path and defeat the whole module.
            map.retain(|_, lock| Arc::strong_count(lock) > 1);
            wanted
                .iter()
                .map(|p| map.entry(p.clone()).or_default().clone())
                .collect()
        };

        let mut guards = Vec::with_capacity(locks.len());
        for lock in locks {
            guards.push(lock.lock_owned().await);
        }
        guards
    }

    /// How many paths the map is currently tracking (tests).
    #[cfg(test)]
    async fn tracked(&self) -> usize {
        self.inner.lock().await.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    fn p(s: &str) -> CanonicalPath {
        CanonicalPath::new_unchecked(s)
    }

    #[tokio::test]
    async fn the_same_path_is_serialised() {
        // The property the module exists for: two tasks contending on one path
        // run one at a time, so the second never reaches coord while the first
        // still holds the lease.
        let locks = Arc::new(PathLocks::new());
        let inside = Arc::new(AtomicUsize::new(0));
        let peak = Arc::new(AtomicUsize::new(0));

        let mut tasks = Vec::new();
        for _ in 0..8 {
            let (locks, inside, peak) = (locks.clone(), inside.clone(), peak.clone());
            tasks.push(tokio::spawn(async move {
                let _g = locks.lock_all(&[p("\\\\srv\\share\\case.yaml")]).await;
                let now = inside.fetch_add(1, Ordering::SeqCst) + 1;
                peak.fetch_max(now, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(5)).await;
                inside.fetch_sub(1, Ordering::SeqCst);
            }));
        }
        for t in tasks {
            t.await.unwrap();
        }
        assert_eq!(peak.load(Ordering::SeqCst), 1, "two tasks were inside at once");
    }

    #[tokio::test]
    async fn different_paths_do_not_block_each_other() {
        // Serialising unrelated files would turn the fan-out into a queue for no
        // reason — the whole point is per-path, not global.
        let locks = PathLocks::new();
        let _a = locks.lock_all(&[p("\\\\srv\\share\\a.md")]).await;
        // Would hang if this waited on `a`.
        let b = tokio::time::timeout(
            Duration::from_millis(250),
            locks.lock_all(&[p("\\\\srv\\share\\b.md")]),
        )
        .await;
        assert!(b.is_ok(), "an unrelated path had to wait");
    }

    #[tokio::test]
    async fn a_repeated_path_in_one_set_does_not_deadlock() {
        // `tokio::sync::Mutex` is not reentrant, so without de-dup this hangs.
        let locks = PathLocks::new();
        let got = tokio::time::timeout(
            Duration::from_millis(250),
            locks.lock_all(&[p("\\\\srv\\share\\a.md"), p("\\\\srv\\share\\a.md")]),
        )
        .await;
        assert!(got.is_ok(), "a duplicated path deadlocked against itself");
        assert_eq!(got.unwrap().len(), 1, "one mutex for one path");
    }

    #[tokio::test]
    async fn a_two_path_set_is_locked_in_a_deterministic_order() {
        // The move verb's deadlock case: opposite orderings must not be able to
        // interleave. Both tasks ask for the same set spelled differently.
        let locks = Arc::new(PathLocks::new());
        let a = p("\\\\srv\\share\\a.md");
        let b = p("\\\\srv\\share\\b.md");

        let mut tasks = Vec::new();
        for i in 0..16 {
            let locks = locks.clone();
            let set = if i % 2 == 0 {
                vec![a.clone(), b.clone()]
            } else {
                vec![b.clone(), a.clone()]
            };
            tasks.push(tokio::spawn(async move {
                let _g = locks.lock_all(&set).await;
                tokio::time::sleep(Duration::from_millis(1)).await;
            }));
        }
        // Deadlock would show up as this timing out rather than as a failure.
        let all = tokio::time::timeout(Duration::from_secs(5), async {
            for t in tasks {
                t.await.unwrap();
            }
        })
        .await;
        assert!(all.is_ok(), "opposite lock orderings deadlocked");
    }

    #[tokio::test]
    async fn released_paths_are_pruned_rather_than_accumulating() {
        let locks = PathLocks::new();
        for i in 0..50 {
            let _g = locks.lock_all(&[p(&format!("\\\\srv\\share\\f{i}.md"))]).await;
        }
        // Each guard dropped at the end of its iteration, so the next call prunes
        // it. One entry may survive: the one taken by the final call.
        let _g = locks.lock_all(&[p("\\\\srv\\share\\last.md")]).await;
        assert!(
            locks.tracked().await <= 2,
            "the map grew to {} entries",
            locks.tracked().await
        );
    }
}
