//! In-memory verified-API-key cache keyed by token digest.
//!
//! bcrypt verification costs ~230ms of pure CPU; paying it per request
//! serialized the whole service (canary-930, live-reproduced). A cache hit
//! skips bcrypt entirely. Entries are keyed by the SHA-256 of the raw bearer
//! token — the raw token is never retained — and expire on a short TTL.
//! Revocation goes through the single admin route in this process, which must
//! call [`AuthCache::invalidate_key_id`] so a revoked key fails on the very
//! next request rather than at TTL expiry.

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use canary_store::VerifiedApiKey;
use sha2::{Digest, Sha256};

/// How long a verified token may be served from cache. Defense-in-depth
/// bound only — explicit invalidation on revoke is the correctness path.
const AUTH_CACHE_TTL_MS: i64 = 300_000;

/// Hard entry bound. The map is cleared when full; legitimate deployments
/// hold a handful of keys, so overflow only happens under token spraying,
/// where dropping the cache is the safe behavior.
const AUTH_CACHE_MAX_ENTRIES: usize = 4096;

struct CacheEntry {
    key: VerifiedApiKey,
    expires_at_ms: i64,
}

/// Process-local verified-key cache shared by all authenticated routes.
#[derive(Default)]
pub(crate) struct AuthCache {
    entries: Mutex<HashMap<[u8; 32], CacheEntry>>,
    /// Bumped on every revocation. An insert staged before a revocation
    /// (candidates fetched, lock dropped, bcrypt still running) is discarded
    /// so a just-revoked key can never enter the cache.
    revocation_epoch: AtomicU64,
}

impl AuthCache {
    /// Snapshot the revocation epoch before fetching verify candidates.
    pub(crate) fn epoch(&self) -> u64 {
        self.revocation_epoch.load(Ordering::Acquire)
    }
    /// Return the cached verified key for `token` when present and fresh.
    pub(crate) fn get(&self, token: &str, now_ms: i64) -> Option<VerifiedApiKey> {
        let digest = token_digest(token);
        let mut entries = self.entries.lock().ok()?;
        match entries.get(&digest) {
            Some(entry) if entry.expires_at_ms > now_ms => Some(entry.key.clone()),
            Some(_) => {
                entries.remove(&digest);
                None
            }
            None => None,
        }
    }

    /// Cache a successfully verified token.
    ///
    /// `fetch_epoch` is the [`AuthCache::epoch`] value read before the
    /// candidates were fetched; the insert is dropped when any revocation
    /// happened since.
    pub(crate) fn insert(&self, token: &str, key: VerifiedApiKey, now_ms: i64, fetch_epoch: u64) {
        let Ok(mut entries) = self.entries.lock() else {
            return;
        };
        if self.revocation_epoch.load(Ordering::Acquire) != fetch_epoch {
            return;
        }
        if entries.len() >= AUTH_CACHE_MAX_ENTRIES {
            entries.clear();
        }
        entries.insert(
            token_digest(token),
            CacheEntry {
                key,
                expires_at_ms: now_ms.saturating_add(AUTH_CACHE_TTL_MS),
            },
        );
    }

    /// Drop every cached token that resolved to `key_id`.
    ///
    /// Called by the revoke route so revocation takes effect on the next
    /// request instead of waiting out the TTL.
    pub(crate) fn invalidate_key_id(&self, key_id: &str) {
        // Revocation must never fail open. `get`/`insert` may fail closed on a
        // poisoned mutex (worst case: a full reverify), but a swallowed purge
        // would leave a revoked key servable for the rest of the TTL. The
        // critical sections only mutate the HashMap, which a panic cannot
        // corrupt structurally — recover the map and heal the poison.
        let mut entries = match self.entries.lock() {
            Ok(entries) => entries,
            Err(poisoned) => {
                self.entries.clear_poison();
                poisoned.into_inner()
            }
        };
        self.revocation_epoch.fetch_add(1, Ordering::AcqRel);
        entries.retain(|_, entry| entry.key.id != key_id);
    }
}

fn token_digest(token: &str) -> [u8; 32] {
    Sha256::digest(token.as_bytes()).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verified(id: &str) -> VerifiedApiKey {
        VerifiedApiKey {
            id: id.to_owned(),
            name: format!("key {id}"),
            scope: "read-only".to_owned(),
            tenant_id: "tenant".to_owned(),
            project_id: "project".to_owned(),
            service: None,
            allow_unbound: true,
        }
    }

    #[test]
    fn hit_within_ttl_miss_after_expiry() {
        let cache = AuthCache::default();
        cache.insert("sk_live_token", verified("KEY-a"), 1_000, cache.epoch());

        let hit = cache.get("sk_live_token", 1_000 + AUTH_CACHE_TTL_MS - 1);
        assert_eq!(hit.map(|key| key.id), Some("KEY-a".to_owned()));

        assert!(
            cache
                .get("sk_live_token", 1_000 + AUTH_CACHE_TTL_MS)
                .is_none()
        );
    }

    #[test]
    fn different_token_never_hits_another_entry() {
        let cache = AuthCache::default();
        cache.insert("sk_live_token", verified("KEY-a"), 0, cache.epoch());
        assert!(cache.get("sk_live_other", 1).is_none());
    }

    #[test]
    #[allow(clippy::unwrap_used, clippy::panic)]
    fn invalidate_purges_and_heals_after_mutex_poisoning() {
        let cache = std::sync::Arc::new(AuthCache::default());
        cache.insert("sk_live_token", verified("KEY-a"), 0, cache.epoch());

        // Poison the entries mutex: panic on a thread holding the guard.
        let poisoner = std::sync::Arc::clone(&cache);
        let _ = std::thread::spawn(move || {
            let _guard = poisoner.entries.lock().unwrap();
            panic!("poison the auth cache mutex");
        })
        .join();

        // Revocation must still purge — and heal the cache for later use.
        cache.invalidate_key_id("KEY-a");
        cache.insert("sk_live_second", verified("KEY-b"), 0, cache.epoch());
        assert!(cache.get("sk_live_token", 1).is_none());
        assert_eq!(
            cache.get("sk_live_second", 1).map(|key| key.id),
            Some("KEY-b".to_owned())
        );
    }

    #[test]
    fn insert_staged_before_a_revocation_is_discarded() {
        let cache = AuthCache::default();
        let fetch_epoch = cache.epoch();
        // A revocation lands while bcrypt is still running out-of-lock.
        cache.invalidate_key_id("KEY-any");
        cache.insert("sk_live_token", verified("KEY-a"), 0, fetch_epoch);
        assert!(cache.get("sk_live_token", 1).is_none());
    }

    #[test]
    fn invalidate_key_id_drops_all_tokens_for_that_key() {
        let cache = AuthCache::default();
        cache.insert("sk_live_token", verified("KEY-a"), 0, cache.epoch());
        cache.insert("sk_live_second", verified("KEY-b"), 0, cache.epoch());

        cache.invalidate_key_id("KEY-a");

        assert!(cache.get("sk_live_token", 1).is_none());
        assert_eq!(
            cache.get("sk_live_second", 1).map(|key| key.id),
            Some("KEY-b".to_owned())
        );
    }
}
