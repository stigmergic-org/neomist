use std::time::Duration;
use tokio::sync::mpsc::Receiver;
use tracing::{info, warn};

use crate::cache;
use crate::ens;
use crate::state::AppState;

pub async fn run_following_loop(state: AppState, mut synced_rx: Receiver<()>) {
    info!("Following: waiting for Helios to finish syncing...");

    if synced_rx.recv().await.is_none() {
        warn!("Following: Helios sync channel closed prematurely");
        return;
    }

    info!("Following: Helios synced, starting background loop");
    loop {
        let interval_mins = {
            let config = state.config.read().await;
            config.following_check_interval_mins
        };

        if interval_mins == 0 {
            tokio::time::sleep(Duration::from_secs(60)).await;
            continue;
        }

        info!("Following: checking for updates on followed domains");

        let domains = match cache::list_cached_domains(&state).await {
            Ok(domains) => domains,
            Err(err) => {
                warn!("Following: failed to list domains: {err}");
                tokio::time::sleep(Duration::from_secs(interval_mins * 60)).await;
                continue;
            }
        };

        let followed_domains = domains.into_iter().filter(|d| d.auto_seeding);

        let provider = state.ens_provider.clone();

        for domain in followed_domains {
            info!("Following: checking {}", domain.domain);
            match ens::resolve_contenthash(&provider, &domain.domain).await {
                Ok(Some(new_cid)) => {
                    let mut needs_update = true;
                    if let Ok(Some(latest_cid)) =
                        ens::latest_cached_cid(&state, &domain.domain).await
                    {
                        if latest_cid == new_cid {
                            needs_update = false;
                        }
                    }

                    if needs_update {
                        info!(
                            "Following: updating {} to new CID {}",
                            domain.domain, new_cid
                        );
                        if let Err(err) =
                            ens::update_mfs_cache(&state, &domain.domain, &new_cid).await
                        {
                            warn!(
                                "Following: failed to update MFS cache for {}: {err}",
                                domain.domain
                            );
                            continue;
                        }
                        if let Err(err) = ens::pin_cid(&state, &new_cid).await {
                            warn!(
                                "Following: failed to pin new CID for {}: {err}",
                                domain.domain
                            );
                        }
                    } else {
                        info!("Following: {} is already up to date", domain.domain);
                    }
                }
                Ok(None) => {
                    warn!(
                        "Following: could not resolve contenthash for {}",
                        domain.domain
                    );
                }
                Err(err) => {
                    warn!(
                        "Following: failed to resolve contenthash for {}: {err}",
                        domain.domain
                    );
                }
            }
        }

        tokio::time::sleep(Duration::from_secs(interval_mins * 60)).await;
    }
}
