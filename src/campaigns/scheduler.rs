// Copyright (c) The Social Proof Foundation, LLC.
// SPDX-License-Identifier: Apache-2.0

use std::time::Duration;

use chrono::Utc;
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

use crate::campaigns::pending_queue::{PendingQueue, scheduler_sleep_duration};
use crate::campaigns::share::process_pending_campaigns;
use crate::state::AppState;

const MIN_SCHEDULER_LOCK_TTL_SECS: u64 = 30;
const IDLE_SLEEP_SECS: u64 = 60;
const LOCK_RENEW_BUFFER_SECS: u64 = 15;

async fn sleep_or_shutdown(duration: Duration, shutdown: &CancellationToken) -> bool {
    if duration.is_zero() {
        return !shutdown.is_cancelled();
    }

    tokio::select! {
        _ = tokio::time::sleep(duration) => !shutdown.is_cancelled(),
        _ = shutdown.cancelled() => false,
    }
}

pub async fn run_scheduler(state: AppState, shutdown: CancellationToken) {
    let mut redis = state.redis.clone();
    let mut holds_lock = false;

    match PendingQueue::migrate_legacy_index(&mut redis).await {
        Ok(count) if count > 0 => info!(migrated = count, "migrated legacy pending campaign index"),
        Ok(_) => {}
        Err(e) => warn!(error = %e, "legacy pending campaign migration failed"),
    }

    loop {
        if shutdown.is_cancelled() {
            break;
        }

        let mut redis = state.redis.clone();
        let acquired = match PendingQueue::try_acquire_scheduler_lock(
            &mut redis,
            MIN_SCHEDULER_LOCK_TTL_SECS,
        )
        .await
        {
            Ok(v) => v,
            Err(e) => {
                warn!(error = %e, "scheduler lock acquisition failed");
                if !sleep_or_shutdown(Duration::from_secs(IDLE_SLEEP_SECS), &shutdown).await {
                    break;
                }
                continue;
            }
        };

        if !acquired {
            if !sleep_or_shutdown(Duration::from_secs(IDLE_SLEEP_SECS), &shutdown).await {
                break;
            }
            continue;
        }

        holds_lock = true;

        if let Err(e) = process_pending_campaigns(&state).await {
            warn!(error = %e, "scheduler process_pending_campaigns failed");
        }

        if shutdown.is_cancelled() {
            break;
        }

        let now = Utc::now();
        let next = match PendingQueue::next_check_after(&mut state.redis.clone()).await {
            Ok(v) => v,
            Err(e) => {
                warn!(error = %e, "scheduler next_check_after failed");
                None
            }
        };

        let sleep_for = scheduler_sleep_duration(now, next, IDLE_SLEEP_SECS);
        let lock_ttl = sleep_for
            .as_secs()
            .max(MIN_SCHEDULER_LOCK_TTL_SECS)
            + LOCK_RENEW_BUFFER_SECS;

        let mut redis = state.redis.clone();
        if let Err(e) = PendingQueue::renew_scheduler_lock(&mut redis, lock_ttl).await {
            warn!(error = %e, "scheduler lock renewal failed");
        }

        if !sleep_or_shutdown(sleep_for, &shutdown).await {
            break;
        }
    }

    if holds_lock {
        let mut redis = state.redis.clone();
        if let Err(e) = PendingQueue::release_scheduler_lock(&mut redis).await {
            warn!(error = %e, "scheduler lock release failed");
        }
    }

    info!("scheduler shutting down");
}
