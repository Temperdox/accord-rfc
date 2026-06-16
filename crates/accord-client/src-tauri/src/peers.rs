//! Yggdrasil peer selection (Settings > Network).
//!
//! Three peer sources, chosen by the user:
//! * **Authorized** - peers we host ([`AUTHORIZED_PEERS`]). Trustworthy, but
//!   connection metadata may be logged per our policies (disclosed in the UI).
//! * **Private** - peers the user hosts themselves (entered in Settings).
//! * **Public** - the community lists from the official
//!   `yggdrasil-network/public-peers` repo (same maintainers as Yggdrasil).
//!
//! For public peers we pick the **best** ones for this user: infer a coarse
//! region from the machine's UTC offset, fetch that region's lists from the
//! repo, then TCP-probe the candidates and rank by real connect latency. Probing
//! beats geolocation - it measures the thing we actually care about - and it
//! doubles as the liveness check (a dead peer never ranks). The mesh watchdog
//! reuses [`probe_many`] to detect dead peers later and migrate.

use std::path::Path;
use std::time::{Duration, Instant};

/// Peers hosted by us. Empty until we stand up infrastructure - the UI says so.
/// Fill (and verify) before a release that advertises the Authorized option.
pub const AUTHORIZED_PEERS: &[&str] = &[];

/// GitHub contents API for the official public-peers repo.
const PEERS_API: &str = "https://api.github.com/repos/yggdrasil-network/public-peers/contents";

/// All region directories in the repo (fallback when region inference is off).
const ALL_REGIONS: &[&str] = &[
    "africa",
    "asia",
    "australia",
    "europe",
    "mena",
    "north-america",
    "south-america",
];

/// How long a probe waits before declaring a peer unreachable.
const PROBE_TIMEOUT: Duration = Duration::from_secs(2);
/// How many ranked public peers to actually configure.
const SELECT_COUNT: usize = 4;

/// File (under the app-data dir) caching the last successful public-peer
/// selection. It lets a later run bootstrap when the repo is unreachable
/// (offline / GitHub blocked or down) without relying on a compiled-in list
/// that goes stale - every successful connect refreshes it with known-good,
/// recently-reachable peers. See [`select_public_peers`].
const PEER_CACHE_FILE: &str = "public-peers-cache.txt";

/// Read peers cached by a previous successful [`select_public_peers`] run.
/// Empty when there's no cache dir, no cache file, or it can't be read. Exposed
/// so the zero-config hosting path (`mesh::load_peers`) can reuse the same
/// known-good set before its compiled-in seed.
pub(crate) fn read_cached_peers(cache_dir: Option<&Path>) -> Vec<String> {
    let Some(dir) = cache_dir else {
        return Vec::new();
    };
    match std::fs::read_to_string(dir.join(PEER_CACHE_FILE)) {
        Ok(text) => text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(str::to_owned)
            .collect(),
        Err(_) => Vec::new(),
    }
}

/// Persist a known-good selection for future repo-down runs. Best-effort: a
/// failed write just means the next offline run falls back to the bundled seed.
fn write_cached_peers(cache_dir: Option<&Path>, peers: &[String]) {
    if let Some(dir) = cache_dir {
        let _ = std::fs::create_dir_all(dir);
        let _ = std::fs::write(dir.join(PEER_CACHE_FILE), peers.join("\n"));
    }
}

/// Coarse region buckets from the local UTC offset. Latency probing makes the
/// final call; this just keeps the candidate fetch small.
fn regions_for_local_offset() -> Vec<&'static str> {
    let offset_hours = chrono::Local::now().offset().local_minus_utc() / 3600;
    match offset_hours {
        -12..=-3 => vec!["north-america", "south-america"],
        -2..=2 => vec!["europe", "africa"],
        3..=5 => vec!["europe", "mena", "asia"],
        6..=14 => vec!["asia", "australia"],
        _ => ALL_REGIONS.to_vec(),
    }
}

/// Extract `tcp://` / `tls://` peer URIs from a markdown document. `quic://`
/// peers are skipped (our probe is TCP and our node build peers over TCP/TLS).
fn extract_peer_uris(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for scheme in ["tls://", "tcp://"] {
        let mut rest = text;
        while let Some(idx) = rest.find(scheme) {
            let tail = &rest[idx..];
            let end = tail
                .find(|c: char| c.is_whitespace() || c == '`' || c == '<' || c == ')' || c == '"')
                .unwrap_or(tail.len());
            let uri = &tail[..end];
            // Sanity: must have a port after the host part.
            if uri.rsplit(':').next().is_some_and(|p| {
                let p = p.split('?').next().unwrap_or(p);
                p.chars().all(|c| c.is_ascii_digit()) && !p.is_empty()
            }) {
                out.push(uri.split('?').next().unwrap_or(uri).to_owned());
            }
            rest = &rest[idx + scheme.len()..];
        }
    }
    out.sort();
    out.dedup();
    out
}

/// `host:port` (socket-address form) from a peer URI, or None if unparseable.
fn socket_addr_of(uri: &str) -> Option<String> {
    let rest = uri.split("://").nth(1)?;
    if rest.starts_with('[') {
        // [ipv6]:port - already in socket-address form.
        rest.contains("]:").then(|| rest.to_owned())
    } else {
        let (host, port) = rest.rsplit_once(':')?;
        Some(format!("{host}:{port}"))
    }
}

/// Fetch the markdown peer lists for `regions` and return all candidate URIs.
///
/// # Errors
/// Returns an error string when the repo cannot be reached at all.
pub async fn fetch_public_peers(regions: &[&str]) -> Result<Vec<String>, String> {
    let client = reqwest::Client::builder()
        .user_agent("accord-client")
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|e| e.to_string())?;

    let mut uris = Vec::new();
    for region in regions {
        // List the region directory, then fetch each country file raw.
        let listing: Vec<serde_json::Value> = match client
            .get(format!("{PEERS_API}/{region}"))
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
        {
            Ok(resp) => resp.json().await.unwrap_or_default(),
            Err(e) => {
                tracing::warn!(region, "could not list public peers: {e}");
                continue;
            }
        };
        for entry in listing {
            let Some(url) = entry.get("download_url").and_then(|u| u.as_str()) else {
                continue;
            };
            if !url.ends_with(".md") {
                continue;
            }
            match client.get(url).send().await {
                Ok(resp) => {
                    if let Ok(text) = resp.text().await {
                        uris.extend(extract_peer_uris(&text));
                    }
                }
                Err(e) => tracing::debug!("peer file fetch failed: {e}"),
            }
        }
    }
    uris.sort();
    uris.dedup();
    if uris.is_empty() {
        return Err("could not fetch any public peers (offline, or GitHub unreachable)".into());
    }
    Ok(uris)
}

/// TCP-probe each peer concurrently; return `(uri, connect latency)` for the
/// reachable ones, fastest first.
pub async fn probe_many(uris: &[String]) -> Vec<(String, Duration)> {
    let probes = uris.iter().cloned().map(|uri| async move {
        let addr = socket_addr_of(&uri)?;
        let started = Instant::now();
        match tokio::time::timeout(PROBE_TIMEOUT, tokio::net::TcpStream::connect(&addr)).await {
            Ok(Ok(_stream)) => Some((uri, started.elapsed())),
            _ => None,
        }
    });
    let mut reachable: Vec<(String, Duration)> = futures::future::join_all(probes)
        .await
        .into_iter()
        .flatten()
        .collect();
    reachable.sort_by_key(|(_, latency)| *latency);
    reachable
}

/// Whether at least one of `uris` is currently reachable (watchdog check).
pub async fn any_reachable(uris: &[String]) -> bool {
    !probe_many(uris).await.is_empty()
}

/// Pick the best public peers for this user: fetch the region's lists, probe,
/// rank by latency, take the top few. Falls back to all regions when the
/// regional candidates are too thin; when the repo is unreachable, prefers the
/// last successful selection cached under `cache_dir` (self-refreshing) and
/// only then the compiled-in seed. A successful selection is written back to
/// the cache. Pass `None` for `cache_dir` to skip caching (e.g. tests).
///
/// # Errors
/// Returns an error string when no peer (fetched, cached, or seeded) is
/// reachable.
pub async fn select_public_peers(cache_dir: Option<&Path>) -> Result<Vec<String>, String> {
    let regions = regions_for_local_offset();
    let mut candidates = fetch_public_peers(&regions).await.unwrap_or_default();
    if candidates.len() < 5 {
        // Region too thin (or fetch failed) - widen to everything.
        if let Ok(all) = fetch_public_peers(ALL_REGIONS).await {
            candidates = all;
        }
    }
    if candidates.is_empty() {
        // Repo unreachable - prefer the last known-good selection (kept fresh by
        // prior successful runs), then the compiled-in seed as a last resort.
        candidates = read_cached_peers(cache_dir);
        if candidates.is_empty() {
            candidates = crate::mesh::default_peers();
        }
    }

    let ranked = probe_many(&candidates).await;
    if ranked.is_empty() {
        return Err("no public peer is reachable from this network".into());
    }
    let selected: Vec<String> = ranked
        .into_iter()
        .take(SELECT_COUNT)
        .map(|(uri, _)| uri)
        .collect();
    // Remember this known-good set so a future repo-down run can reuse it.
    write_cached_peers(cache_dir, &selected);
    Ok(selected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_and_dedupes_uris() {
        let md = "* `tls://01.ffm.deu.example:443` ok\n- tcp://192.0.2.7:9001 \
                  and quic://skip.me:1 plus tls://[2001:db8::1]:443?key=abc again \
                  tcp://192.0.2.7:9001";
        let uris = extract_peer_uris(md);
        assert_eq!(
            uris,
            vec![
                "tcp://192.0.2.7:9001".to_owned(),
                "tls://01.ffm.deu.example:443".to_owned(),
                "tls://[2001:db8::1]:443".to_owned(),
            ]
        );
    }

    #[test]
    fn peer_cache_round_trips_and_tolerates_missing() {
        let dir = std::env::temp_dir().join(format!("accord-peers-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        // No cache file yet -> empty, no panic.
        assert!(read_cached_peers(Some(&dir)).is_empty());
        // No cache dir at all -> empty, no panic.
        assert!(read_cached_peers(None).is_empty());

        let peers = vec![
            "tls://a.example:443".to_owned(),
            "tcp://b.example:9001".to_owned(),
        ];
        write_cached_peers(Some(&dir), &peers);
        assert_eq!(read_cached_peers(Some(&dir)), peers);

        // Blank lines / surrounding whitespace are ignored on read.
        std::fs::write(dir.join(PEER_CACHE_FILE), "  tls://c.example:443  \n\n").unwrap();
        assert_eq!(
            read_cached_peers(Some(&dir)),
            vec!["tls://c.example:443".to_owned()]
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn socket_addr_forms() {
        assert_eq!(
            socket_addr_of("tls://host.example:443").as_deref(),
            Some("host.example:443")
        );
        assert_eq!(
            socket_addr_of("tcp://[2001:db8::1]:9001").as_deref(),
            Some("[2001:db8::1]:9001")
        );
        assert_eq!(socket_addr_of("garbage"), None);
    }
}
