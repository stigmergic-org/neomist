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

        for domain in followed_domains {
            info!("Following: checking {}", domain.domain);
            match ens::resolve_contenthash(&state, &domain.domain).await {
                Ok(Some(metadata)) => {
                    if let Err(err) =
                        cache::write_contenthash_metadata(&state, &domain.domain, &metadata).await
                    {
                        warn!(
                            "Following: failed to write contenthash metadata for {}: {err}",
                            domain.domain
                        );
                    }

                    match ens::update_mfs_cache(&state, &domain.domain, &metadata.contenthash).await {
                        Ok(true) => {
                            info!(
                                "Following: updating {} to {}",
                                domain.domain,
                                metadata.contenthash.target()
                            );
                            if let Err(err) = ens::pin_content(&state, &metadata.contenthash).await {
                                warn!(
                                    "Following: failed to pin new content for {}: {err}",
                                    domain.domain
                                );
                            }
                        }
                        Ok(false) => {
                            info!("Following: {} is already up to date", domain.domain);
                        }
                        Err(err) => {
                            warn!(
                                "Following: failed to update MFS cache for {}: {err}",
                                domain.domain
                            );
                        }
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
